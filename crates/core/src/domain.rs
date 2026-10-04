//! Shared domain enums, stored in SQLite as lowercase TEXT and exchanged with
//! the frontend as JSON strings. Kept dependency-free so both server and
//! mobile can use them.

use serde::{Deserialize, Serialize};

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }
        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

str_enum!(
    /// Lifecycle of a sighting (a user's single foraging observation).
    SightingStatus {
        Open => "open",
        Archived => "archived",
    }
);

str_enum!(
    /// Outcome of the fast triage pass. `Insufficient` means the model could
    /// not commit to even a genus and is asking for more photos/info — this
    /// must always be an available, honest answer, never forced into a guess.
    TriageStatus {
        Insufficient => "insufficient",
        GenusCandidate => "genus_candidate",
        SpeciesCandidate => "species_candidate",
    }
);

str_enum!(
    /// How dangerous a species (or a look-alike) is to get wrong.
    DangerLevel {
        Unknown => "unknown",
        Safe => "safe",
        Caution => "caution",
        Toxic => "toxic",
        DeadlyToxic => "deadly_toxic",
    }
);

/// Standard safety disclaimer. Every API response and UI surface that shows a
/// species guess must include this text verbatim — never summarized or
/// softened. See `docs/ARCHITECTURE.md`.
pub const SAFETY_DISCLAIMER: &str = "Forage Buddy gives a best-effort AI guess, not a confirmed identification. Never eat, use, or handle anything based solely on this app. Misidentifying a wild plant or fungus can cause severe illness or death. Confirm with a qualified local expert, a spore print, and multiple field guides before consuming or using anything you forage.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_str() {
        assert_eq!(
            TriageStatus::parse("genus_candidate"),
            Some(TriageStatus::GenusCandidate)
        );
        assert_eq!(TriageStatus::GenusCandidate.as_str(), "genus_candidate");
        assert_eq!(DangerLevel::parse("deadly_toxic"), Some(DangerLevel::DeadlyToxic));
        assert_eq!(SightingStatus::parse("nope"), None);
    }

    #[test]
    fn serde_uses_snake_case() {
        let j = serde_json::to_string(&DangerLevel::DeadlyToxic).unwrap();
        assert_eq!(j, "\"deadly_toxic\"");
    }
}
