//! Limit profiles (EVOL Annex B) as constant data. The table below is also parsed by a node test that
//! compares it with `host/core/limits.js`, so keep one row per line in the form `L("name", [normal,
//! hardened, contain, micro]),`.
use crate::mem::LimitId;

const MIB: u64 = 1 << 20;

/// Column order of the rows: `normal`, `hardened`, `contain`, `micro`.
#[allow(non_snake_case)]
const fn L(name: &'static str, v: [u64; 4]) -> Row {
    Row { name, v }
}

/// One named limit with its value in each profile.
pub struct Row {
    pub name: &'static str,
    pub v: [u64; 4],
}

pub const LIMITS: &[Row] = &[
    L("maxSourceBytes", [64 * MIB, 8 * MIB, 8 * MIB, 4 * MIB]),
    L("maxExecDepth", [64, 64, 64, 64]),
    L("maxCallDepth", [100, 100, 100, 100]),
    L("maxSubstitutionDepth", [50, 50, 50, 50]),
    L("maxSourceDepth", [100, 100, 100, 100]),
    L("maxCommandCount", [100000, 10000, 100000, 25000]),
    L("maxLoopIterations", [100000, 10000, 100000, 25000]),
    L("maxAwkIterations", [100000, 10000, 100000, 10000]),
    L("maxSedIterations", [100000, 10000, 100000, 10000]),
    L("maxJqIterations", [10000000, 10000, 10000000, 10000]),
    L("maxWorkUnits", [100000000, 100000, 100000000, 1000000]),
    L("maxQueryTokens", [100000, 25000, 100000, 25000]),
    L("maxQueryDepth", [1000, 256, 1000, 256]),
    L("maxQueryElements", [1000000, 100000, 1000000, 100000]),
    L("maxAwkParserTokens", [100000, 25000, 100000, 25000]),
    L("maxAwkParserDepth", [256, 128, 256, 128]),
    L("maxAwkParserOperations", [1000000, 100000, 1000000, 100000]),
    L("maxCsvRows", [1000000, 100000, 1000000, 100000]),
    L("maxCsvCells", [10000000, 1000000, 10000000, 1000000]),
    L("maxTraversalEntries", [1000000, 100000, 1000000, 100000]),
    L("maxTraversalDepth", [1000, 256, 1000, 256]),
    L("maxTraversalWork", [1000000, 100000, 1000000, 100000]),
    L("maxGlobOperations", [1000000, 100000, 1000000, 100000]),
    L("maxArrayElements", [1000000, 100000, 1000000, 100000]),
    L("maxBraceExpansionResults", [100000, 10000, 100000, 10000]),
    L("maxFileDescriptors", [4096, 1024, 4096, 1024]),
    L("maxLiveBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]),
    L("maxInputBytes", [512 * MIB, 32 * MIB, 64 * MIB, 16 * MIB]),
    L("maxFileSystemBytes", [1024 * MIB, 128 * MIB, 128 * MIB, 32 * MIB]),
    L("maxOutputSize", [256 * MIB, 10 * MIB, 32 * MIB, 8 * MIB]),
    L("maxStringLength", [64 * MIB, 10 * MIB, 16 * MIB, 4 * MIB]),
    L("maxHeredocSize", [64 * MIB, 10 * MIB, 16 * MIB, 4 * MIB]),
    L("maxArchiveBytes", [1024 * MIB, 128 * MIB, 128 * MIB, 32 * MIB]),
    L("maxArchiveCompressedBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]),
    L("maxArchiveEntryBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]),
    L("maxArchiveEntries", [1000000, 100000, 100000, 100000]),
    L("maxExecutionTimeMs", [3600000, 30000, 60000, 10000]),
    L("maxExtensionCleanupTimeMs", [100, 25, 100, 25]),
    // Accepted and unused in v1 (EVOL Annex B); the values are placeholders.
    L("maxDatabaseBytes", [0, 0, 0, 0]),
    L("maxDatabaseResultBytes", [0, 0, 0, 0]),
    L("maxWorkerMessageBytes", [0, 0, 0, 0]),
    L("maxSqliteTimeoutMs", [0, 0, 0, 0]),
    L("maxPythonTimeoutMs", [0, 0, 0, 0]),
    L("maxJsTimeoutMs", [0, 0, 0, 0]),
    L("maxJsBridgeRequests", [0, 0, 0, 0]),
];

/// Number of named limits.
pub const COUNT: usize = LIMITS.len();

// Indices of the limits the kernel reads itself (checked by a unit test against the names).
pub const I_SOURCE_BYTES: usize = 0;
pub const I_EXEC_DEPTH: usize = 1;
pub const I_CALL_DEPTH: usize = 2;
pub const I_COMMAND_COUNT: usize = 5;
pub const I_WORK_UNITS: usize = 10;
pub const I_ARRAY_ELEMENTS: usize = 23;
pub const I_LIVE_BYTES: usize = 26;
pub const I_INPUT_BYTES: usize = 27;
pub const I_FS_BYTES: usize = 28;
pub const I_OUTPUT_SIZE: usize = 29;
pub const I_STRING_LENGTH: usize = 30;

/// Per-profile constants: headroom, ceiling and `exec-oneshot` cap, in bytes. Order of `Profile`:
/// `micro`, `contain`, `hardened`, `normal` (the WIT enum order).
pub struct ProfileData {
    pub headroom: u64,
    pub ceiling: u64,
    pub oneshot: u64,
}

pub const PROFILES: [ProfileData; 4] = [
    ProfileData { headroom: 8 * MIB, ceiling: 64 * MIB, oneshot: 4 * MIB },
    ProfileData { headroom: 32 * MIB, ceiling: 256 * MIB, oneshot: 16 * MIB },
    ProfileData { headroom: 54 * MIB, ceiling: 256 * MIB, oneshot: 27 * MIB },
    ProfileData { headroom: 64 * MIB, ceiling: 1856 * MIB, oneshot: 32 * MIB },
];

/// Column of `LIMITS` for a WIT profile index (`micro`, `contain`, `hardened`, `normal`).
const fn column(profile: usize) -> usize {
    match profile {
        0 => 3,
        1 => 2,
        2 => 1,
        _ => 0,
    }
}

/// The limits of one shell: a profile resolved with its overrides.
#[derive(Clone, Copy)]
pub struct Resolved {
    pub profile: usize,
    pub vals: [u64; COUNT],
}

impl Resolved {
    pub fn of_profile(profile: usize) -> Resolved {
        let c = column(profile);
        let mut vals = [0u64; COUNT];
        let mut i = 0;
        while i < COUNT {
            vals[i] = LIMITS[i].v[c];
            i += 1;
        }
        Resolved { profile, vals }
    }

    /// Sets a limit by name; false when the name is unknown.
    pub fn set(&mut self, name: &str, value: u64) -> bool {
        match index_of(name) {
            Some(i) => {
                self.vals[i] = value;
                true
            }
            None => false,
        }
    }

    pub fn headroom(&self) -> u64 {
        PROFILES[self.profile].headroom
    }

    /// `maxFileSystemBytes + maxLiveBytes + maxOutputSize + headroom`.
    pub fn byte_sum(&self) -> u64 {
        self.vals[I_FS_BYTES]
            .saturating_add(self.vals[I_LIVE_BYTES])
            .saturating_add(self.vals[I_OUTPUT_SIZE])
            .saturating_add(self.headroom())
    }
}

pub fn index_of(name: &str) -> Option<usize> {
    LIMITS.iter().position(|r| r.name == name)
}

/// `LimitId` of table index `i` (ids 0 and 1 belong to the memory module).
pub const fn id_of_index(i: usize) -> LimitId {
    LimitId(i as u16 + 2)
}

/// Name carried by `exec-result.limit-name` for a `LimitId`.
pub fn name_of(id: LimitId) -> &'static str {
    match id.0 {
        0 => "memory-ceiling",
        1 => "headroom",
        n => match LIMITS.get(n as usize - 2) {
            Some(r) => r.name,
            None => "limit",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_match_names() {
        assert_eq!(LIMITS[I_SOURCE_BYTES].name, "maxSourceBytes");
        assert_eq!(LIMITS[I_LIVE_BYTES].name, "maxLiveBytes");
        assert_eq!(LIMITS[I_INPUT_BYTES].name, "maxInputBytes");
        assert_eq!(LIMITS[I_FS_BYTES].name, "maxFileSystemBytes");
        assert_eq!(LIMITS[I_OUTPUT_SIZE].name, "maxOutputSize");
    }

    #[test]
    fn profile_sums_fit_their_ceilings() {
        for p in 0..4 {
            let r = Resolved::of_profile(p);
            assert_eq!(r.byte_sum(), PROFILES[p].ceiling, "profile {p}");
        }
    }

    #[test]
    fn ids_round_trip() {
        assert_eq!(name_of(id_of_index(I_LIVE_BYTES)), "maxLiveBytes");
        assert_eq!(name_of(LimitId::CEILING), "memory-ceiling");
    }
}
