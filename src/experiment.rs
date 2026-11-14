//! Experiment arms: each session is assigned, from a hash of its id, to the
//! old or the new behaviour of each toggle under test, so `jouet stats` can
//! compare the two on the user's own sessions.
//!
//! The toggles are the stop gate (blocking, the old behaviour, or observing,
//! the new default) and the `python-edit` rule (denying, the old behaviour,
//! or off, the new default). `[experiment] control_share` percent of
//! sessions keep the old behaviour of each toggle, assigned independently,
//! so one session can be in the control arm of one toggle and not the
//! other. The assignment needs no state: every hook of a session computes
//! the same arm from its id, and the session-start entry in the ledger
//! records it.
//!
//! A setting that already turns the old behaviour on (`[hooks] gate =
//! "block"`, `[rules] enable = ["python-edit"]`) applies to every session,
//! whatever its arm, so both arms then behave alike.
//!
//! The drop toggle runs the other way: it is new behaviour on trial, not an
//! old default kept. `[experiment] drop_share` percent of Claude Code
//! sessions (the `new` arm) have the desktop app's
//! Auto-fix events that are only noise (`crate::prompt::noise`) blocked
//! before the agent sees them; the rest (the `control` arm) get every event.
//! It is 0, off, by default.

use std::collections::BTreeMap;

/// The toggles under test, as the ledger names them.
pub(crate) const GATE: &str = "gate";
pub(crate) const PYTHON_EDIT: &str = "python-edit";
pub(crate) const DROP: &str = "drop";

/// Which arm of each toggle a session is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Arm {
    /// The stop gate blocks, as before observe mode.
    pub gate_blocks: bool,
    /// The python-edit rule denies, as before it was off by default.
    pub python_edit: bool,
    /// Auto-fix events that are only noise are blocked before the agent
    /// sees them.
    pub drop: bool,
}

impl Arm {
    /// The arm of `session` when `control` percent of sessions (0 to 100)
    /// keep each old default and `drop` percent have noise dropped.
    pub(crate) fn of(session: &str, control: u64, drop: u64) -> Self {
        let within = |toggle: &str, share: u64| bucket(toggle, session) < share.min(100);
        Self {
            gate_blocks: within(GATE, control),
            python_edit: within(PYTHON_EDIT, control),
            drop: within(DROP, drop),
        }
    }

    /// For the ledger: the arm, `control` (today's behaviour) or `new`, of
    /// each toggle whose share is above 0.
    pub(crate) fn labels(self, control: u64, drop: u64) -> BTreeMap<String, String> {
        let label = |old: bool| if old { "control" } else { "new" }.to_string();
        let mut labels = BTreeMap::new();
        if control > 0 {
            labels.insert(GATE.to_string(), label(self.gate_blocks));
            labels.insert(PYTHON_EDIT.to_string(), label(self.python_edit));
        }
        if drop > 0 {
            labels.insert(DROP.to_string(), label(!self.drop));
        }
        labels
    }
}

/// A session's place, 0 to 99, for one toggle: SHA-256 of the toggle and
/// the id, so it is the same in every process and every release, and each
/// toggle is assigned independently of the other.
fn bucket(toggle: &str, session: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::new()
        .chain_update(toggle)
        .chain_update([0])
        .chain_update(session)
        .finalize();
    let mut first = [0; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first) % 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_keeps_its_arm_and_the_share_holds_over_many() {
        let id = "3f0c2a8e-1b7d-4c55-9e2a-0d6f1b9c7e41";
        assert_eq!(Arm::of(id, 25, 25), Arm::of(id, 25, 25));
        assert_eq!(Arm::of(id, 0, 0), Arm::default());
        assert_eq!(
            Arm::of(id, 100, 0),
            Arm {
                gate_blocks: true,
                python_edit: true,
                drop: false,
            }
        );
        assert_eq!(
            Arm::of(id, 0, 100),
            Arm {
                drop: true,
                ..Arm::default()
            }
        );
        let arms: Vec<Arm> = (0..10_000)
            .map(|n| Arm::of(&format!("session-{n}"), 25, 10))
            .collect();
        let gate = arms.iter().filter(|a| a.gate_blocks).count();
        let python = arms.iter().filter(|a| a.python_edit).count();
        let both = arms
            .iter()
            .filter(|a| a.gate_blocks && a.python_edit)
            .count();
        assert!((2_300..2_700).contains(&gate), "{gate}");
        assert!((2_300..2_700).contains(&python), "{python}");
        // Assigned independently: about a quarter of a quarter.
        assert!((450..800).contains(&both), "{both}");
        let drop = arms.iter().filter(|a| a.drop).count();
        assert!((800..1_200).contains(&drop), "{drop}");
        // Also independent of the gate: a tenth of a quarter, not all of a tenth.
        let gate_and_drop = arms.iter().filter(|a| a.gate_blocks && a.drop).count();
        assert!((150..400).contains(&gate_and_drop), "{gate_and_drop}");
        // The hash is fixed, so a session keeps its arm across releases.
        assert_eq!(bucket(GATE, "s1"), bucket(GATE, "s1"));
        assert_ne!(bucket(GATE, "s1"), bucket(PYTHON_EDIT, "s1"));
    }

    #[test]
    fn the_ledger_names_each_toggles_arm() {
        let arm = Arm {
            gate_blocks: true,
            python_edit: false,
            drop: false,
        };
        assert_eq!(
            arm.labels(25, 25),
            BTreeMap::from([
                ("drop".to_string(), "control".to_string()),
                ("gate".to_string(), "control".to_string()),
                ("python-edit".to_string(), "new".to_string()),
            ])
        );
        // A toggle with no share is not under test, so it has no arm.
        assert_eq!(
            arm.labels(0, 25),
            BTreeMap::from([("drop".to_string(), "control".to_string())])
        );
        assert!(arm.labels(0, 0).is_empty());
    }
}
