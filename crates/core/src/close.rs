//! Close states (§6.8): only an adjudicated close is final.

use crate::rules::PartyIndex;

/// What the fold concluded about an `abandoned` close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseState {
    /// The witness quorum agrees every subject was silent past the allowance. Final.
    AdjudicatedValid,
    /// The witness quorum agrees a subject was not silent long enough. Final: dropped.
    AdjudicatedInvalid,
    /// Fewer than a quorum of engaged witnesses have receipts here, or they disagree, and no
    /// subject has a record at this seq. Provisional.
    Asserted,
    /// Asserted, and some subject has a record at this seq. Provisional; two parties' words.
    Contested {
        /// The subjects who were present.
        present: Vec<PartyIndex>,
    },
    /// Void by construction: a subject was present and the close is not adjudicated late, the
    /// subject bound was exceeded, or the author is a subject. Anomaly against the author.
    Void {
        /// Why.
        reason: String,
    },
}

impl CloseState {
    /// Does this state end the chain?
    pub fn is_final(&self) -> bool {
        matches!(self, CloseState::AdjudicatedValid)
    }

    /// Does this state pause the chain (provisional, neither ended nor dropped)?
    pub fn is_provisional(&self) -> bool {
        matches!(self, CloseState::Asserted | CloseState::Contested { .. })
    }
}

/// The `|subject|` bound (§6.8): `N − 1` when adjudicated, `max(1, N − 2)` when asserted, so a
/// two-party chain can always be asserted closed.
pub fn max_subjects(n: usize, adjudicated: bool) -> usize {
    if adjudicated {
        n.saturating_sub(1)
    } else {
        n.saturating_sub(2).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_party_chain_can_always_be_asserted_closed() {
        assert_eq!(max_subjects(2, false), 1);
        assert_eq!(max_subjects(2, true), 1);
    }

    #[test]
    fn three_parties_need_two_present_to_assert() {
        assert_eq!(max_subjects(3, false), 1);
        assert_eq!(max_subjects(3, true), 2);
    }
}
