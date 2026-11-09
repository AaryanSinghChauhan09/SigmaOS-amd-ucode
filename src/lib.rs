// SPDX-License-Identifier: MIT
// SigmaOS-amd-ucode - AMD CPU Microcode Updates component
// amd-ucode-style early microcode loading for AMD processors,
// inspired by Arch's amd-ucode package.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

// ============================================================================
// CPU MICROCODE INFO
// ============================================================================

/// AMD CPU family/model info
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuId {
    /// Family ID (e.g. 0x17 = Zen)
    pub family: u8,
    /// Model ID
    pub model: u8,
    /// Stepping
    pub stepping: u8,
}

impl CpuId {
    /// Parse from CPUID leaf 1 EBX/EDX (simplified)
    pub fn from_cpuid(eax: u32) -> Self {
        // Family = bits 8-11 + bits 20-27 (extended)
        // Model = bits 4-7 + bits 16-19 (extended)
        let base_family = ((eax >> 8) & 0xF) as u8;
        let base_model = ((eax >> 4) & 0xF) as u8;
        let ext_family = ((eax >> 20) & 0xFF) as u8;
        let ext_model = ((eax >> 16) & 0xF) as u8;
        let family = if base_family == 0xF { base_family + ext_family } else { base_family };
        let model = if base_family == 0xF || base_family == 0x6 {
            (ext_model << 4) | base_model
        } else {
            base_model
        };
        Self { family, model, stepping: (eax & 0xF) as u8 }
    }

    /// Human-readable CPU name guess
    pub fn cpu_name(&self) -> &'static str {
        match (self.family, self.model) {
            (0x17, 0x00..=0x0F) => "AMD Zen/Zen+",
            (0x17, 0x30..=0x3F) => "AMD Zen 2",
            (0x17, 0x20..=0x2F) | (0x17, 0x70..=0x7F) | (0x17, 0xA0..=0xAF) => "AMD Zen 3",
            (0x19, 0x00..=0x0F) | (0x19, 0x10..=0x4F) => "AMD Zen 4",
            _ => "AMD CPU",
        }
    }

    /// Whether this CPU needs microcode update
    pub fn needs_ucode(&self) -> bool {
        // All AMD CPUs since family 0x10 (Barcelona) can use microcode updates
        self.family >= 0x10
    }
}

/// Microcode patch level info
#[derive(Debug, Clone)]
pub struct MicrocodeInfo {
    /// Current microcode patch level
    pub patch_level: u32,
    /// Date of microcode (YYYYMMDD format)
    pub date: u32,
    /// CPU signature this microcode applies to
    pub cpu_signature: u32,
    /// Whether this is the latest
    pub is_latest: bool,
}

/// A microcode update file
#[derive(Debug, Clone)]
pub struct MicrocodeUpdate {
    /// CPU signature this applies to
    pub cpu_signature: u32,
    /// Patch level
    pub patch_level: u32,
    /// Date (YYYYMMDD)
    pub date: u32,
    /// Binary microcode data
    pub data: Vec<u8>,
    /// Whether this is a late-load update (vs early-load)
    pub late_load: bool,
}

impl MicrocodeUpdate {
    /// Whether this update applies to a given CPU
    pub fn applies_to(&self, cpu: &CpuId) -> bool {
        self.cpu_signature == cpu_signature(cpu)
    }

    /// Whether this update is newer than current
    pub fn is_newer(&self, current: &MicrocodeInfo) -> bool {
        self.patch_level > current.patch_level ||
        (self.patch_level == current.patch_level && self.date > current.date)
    }
}

// ============================================================================
// MICROCODE MANAGER
// ============================================================================

/// Microcode update manager
pub struct UcodeManager {
    /// Available updates
    updates: Mutex<Vec<MicrocodeUpdate>>,
    /// Current microcode per CPU (signature -> info)
    current: Mutex<HashMap<u32, MicrocodeInfo>>,
    /// Whether early loading is enabled (initramfs)
    early_load: Mutex<bool>,
    /// Whether late loading is enabled (runtime)
    late_load: Mutex<bool>,
}

impl UcodeManager {
    pub fn new() -> Self {
        Self {
            updates: Mutex::new(Vec::new()),
            current: Mutex::new(HashMap::new()),
            early_load: Mutex::new(true),  // recommended
            late_load: Mutex::new(true),   // fallback
        }
    }

    /// Add a microcode update
    pub fn add_update(&self, update: MicrocodeUpdate) {
        self.updates.lock().unwrap().push(update);
    }

    /// Get current microcode for a CPU signature
    pub fn current_ucode(&self, signature: u32) -> Option<MicrocodeInfo> {
        self.current.lock().unwrap().get(&signature).cloned()
    }

    /// Set current microcode level
    pub fn set_current(&self, signature: u32, patch_level: u32, date: u32) {
        self.current.lock().unwrap().insert(signature, MicrocodeInfo {
            patch_level,
            date,
            cpu_signature: signature,
            is_latest: false,
        });
    }

    /// Find best update for a CPU
    pub fn best_update(&self, cpu: &CpuId) -> Option<MicrocodeUpdate> {
        let signature = cpu_signature(cpu);
        let current = self.current_ucode(signature);
        let updates = self.updates.lock().unwrap();
        updates.iter()
            .filter(|u| u.applies_to(cpu))
            .filter(|u| current.as_ref().map(|c| u.is_newer(c)).unwrap_or(true))
            .max_by_key(|u| (u.patch_level, u.date))
            .cloned()
    }

    /// Check if CPU needs update
    pub fn needs_update(&self, cpu: &CpuId) -> bool {
        self.best_update(cpu).is_some()
    }

    /// Get all updates for a CPU
    pub fn updates_for_cpu(&self, cpu: &CpuId) -> Vec<MicrocodeUpdate> {
        self.updates.lock().unwrap().iter()
            .filter(|u| u.applies_to(cpu))
            .cloned()
            .collect()
    }

    /// Enable/disable early loading
    pub fn set_early_load(&self, enabled: bool) {
        *self.early_load.lock().unwrap() = enabled;
    }

    pub fn early_load_enabled(&self) -> bool {
        *self.early_load.lock().unwrap()
    }

    /// Enable/disable late loading
    pub fn set_late_load(&self, enabled: bool) {
        *self.late_load.lock().unwrap() = enabled;
    }

    pub fn late_load_enabled(&self) -> bool {
        *self.late_load.lock().unwrap()
    }

    /// Generate initramfs microcode inclusion recommendation
    pub fn initramfs_recommendation(&self) -> &'static str {
        if self.early_load_enabled() {
            "Include amd-ucode in initramfs before other hooks"
        } else {
            "Early loading disabled; late loading will apply at runtime"
        }
    }

    /// List all available updates
    pub fn list_updates(&self) -> Vec<MicrocodeUpdate> {
        self.updates.lock().unwrap().clone()
    }

    /// Count updates
    pub fn update_count(&self) -> usize {
        self.updates.lock().unwrap().len()
    }
}

impl Default for UcodeManager {
    fn default() -> Self { Self::new() }
}

/// Compute CPU signature from family/model/stepping
fn cpu_signature(cpu: &CpuId) -> u32 {
    // Simplified signature: family(8) | model(4) | stepping(4) in high bits
    ((cpu.family as u32) << 24) | ((cpu.model as u32) << 16) | ((cpu.stepping as u32) << 8)
}

#[derive(Debug)]
pub enum UcodeError {
    NoUpdateForCpu,
    AlreadyLatest,
    LoadFailed(String),
    NotAmdCpu,
}

impl fmt::Display for UcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoUpdateForCpu => write!(f, "no microcode update for this CPU"),
            Self::AlreadyLatest => write!(f, "microcode already up to date"),
            Self::LoadFailed(m) => write!(f, "load failed: {}", m),
            Self::NotAmdCpu => write!(f, "not an AMD CPU"),
        }
    }
}
impl std::error::Error for UcodeError {}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn zen3_cpu() -> CpuId {
        CpuId { family: 0x17, model: 0x21, stepping: 0x2 } // Zen 3 (Vermeer)
    }

    fn zen4_cpu() -> CpuId {
        CpuId { family: 0x19, model: 0x11, stepping: 0x0 } // Zen 4
    }

    fn make_update(signature: u32, patch: u32, date: u32) -> MicrocodeUpdate {
        MicrocodeUpdate {
            cpu_signature: signature,
            patch_level: patch,
            date,
            data: vec![0xAB; 64],
            late_load: false,
        }
    }

    #[test]
    fn test_cpu_signature() {
        let cpu = zen3_cpu();
        let sig = cpu_signature(&cpu);
        assert!(sig > 0);
        // Same CPU should give same signature
        assert_eq!(sig, cpu_signature(&zen3_cpu()));
    }

    #[test]
    fn test_cpu_needs_ucode() {
        assert!(zen3_cpu().needs_ucode());
        assert!(zen4_cpu().needs_ucode());
        // Very old CPU (family < 0x10) doesn't need it
        let old = CpuId { family: 0x05, model: 0x04, stepping: 0x0 };
        assert!(!old.needs_ucode());
    }

    #[test]
    fn test_cpu_name() {
        assert!(zen3_cpu().cpu_name().contains("Zen"));
        assert!(zen4_cpu().cpu_name().contains("Zen"));
    }

    #[test]
    fn test_best_update() {
        let mgr = UcodeManager::new();
        let cpu = zen3_cpu();
        let sig = cpu_signature(&cpu);

        // Add older update
        mgr.add_update(make_update(sig, 0x01000000, 20200101));
        // Add newer update
        mgr.add_update(make_update(sig, 0x01000001, 20210101));
        // Add for different CPU
        mgr.add_update(make_update(cpu_signature(&zen4_cpu()), 0x02000000, 20220101));

        let best = mgr.best_update(&cpu).unwrap();
        assert_eq!(best.patch_level, 0x01000001); // newest for this CPU
        assert_eq!(best.date, 20210101);
    }

    #[test]
    fn test_needs_update() {
        let mgr = UcodeManager::new();
        let cpu = zen3_cpu();
        let sig = cpu_signature(&cpu);
        mgr.add_update(make_update(sig, 0x01000001, 20210101));
        assert!(mgr.needs_update(&cpu)); // no current → needs update

        // Set current to latest
        mgr.set_current(sig, 0x01000001, 20210101);
        assert!(!mgr.needs_update(&cpu)); // already latest
    }

    #[test]
    fn test_early_late_load() {
        let mgr = UcodeManager::new();
        assert!(mgr.early_load_enabled());
        assert!(mgr.late_load_enabled());
        mgr.set_early_load(false);
        assert!(!mgr.early_load_enabled());
        assert!(mgr.initramfs_recommendation().contains("disabled"));
    }

    #[test]
    fn test_updates_for_cpu() {
        let mgr = UcodeManager::new();
        let cpu = zen3_cpu();
        let sig = cpu_signature(&cpu);
        mgr.add_update(make_update(sig, 1, 20200101));
        mgr.add_update(make_update(sig, 2, 20210101));
        mgr.add_update(make_update(cpu_signature(&zen4_cpu()), 3, 20220101));

        let updates = mgr.updates_for_cpu(&cpu);
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|u| u.applies_to(&cpu)));
    }

    #[test]
    fn test_update_is_newer() {
        let current = MicrocodeInfo {
            patch_level: 100,
            date: 20200101,
            cpu_signature: 0,
            is_latest: false,
        };
        let newer = MicrocodeUpdate {
            cpu_signature: 0, patch_level: 101, date: 20200101,
            data: vec![], late_load: false,
        };
        assert!(newer.is_newer(&current));

        let same = MicrocodeUpdate {
            cpu_signature: 0, patch_level: 100, date: 20200101,
            data: vec![], late_load: false,
        };
        assert!(!same.is_newer(&current));
    }
}
