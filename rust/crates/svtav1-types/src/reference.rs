//! Reference frame definitions.
//!
//! Ported from `definitions.h` lines 1317-1379.

/// Reference frame identifiers.
///
/// Note: In C, MvReferenceFrame is just i8. We use an enum for type safety
/// but provide i8 conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i8)]
pub enum ReferenceFrame {
    None = -1,
    Intra = 0,
    Last = 1,
    Last2 = 2,
    Last3 = 3,
    Golden = 4,
    BwdRef = 5,
    AltRef2 = 6,
    AltRef = 7,
}

impl ReferenceFrame {
    #[inline]
    pub const fn as_i8(self) -> i8 {
        self as i8
    }
}

/// Number of reference frame slots (REF_FRAMES).
pub const REF_FRAMES: usize = 8;

/// Log2 of REF_FRAMES.
pub const REF_FRAMES_LOG2: usize = 3;

/// Number of references per frame in the bitstream.
pub const REFS_PER_FRAME: usize = 7;

/// Number of inter (non-intra) reference types.
pub const INTER_REFS_PER_FRAME: usize = 7; // ALTREF - LAST + 1

/// Total refs per frame (including INTRA_FRAME).
pub const TOTAL_REFS_PER_FRAME: usize = 8; // ALTREF - INTRA + 1

/// Number of forward reference types.
pub const FWD_REFS: usize = 4; // GOLDEN - LAST + 1

/// Number of backward reference types.
pub const BWD_REFS: usize = 3; // ALTREF - BWDREF + 1

/// Number of single references.
pub const SINGLE_REFS: usize = FWD_REFS + BWD_REFS;

/// Uni-directional compound reference pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum UniDirCompRef {
    LastLast2 = 0,
    LastLast3 = 1,
    LastGolden = 2,
    BwdRefAltRef = 3,
    Last2Last3 = 4,
    Last2Golden = 5,
    Last3Golden = 6,
    BwdRefAltRef2 = 7,
    AltRef2AltRef = 8,
}

/// Total uni-directional compound reference types.
pub const TOTAL_UNIDIR_COMP_REFS: usize = 9;

/// Number of explicitly signaled uni-directional compound refs.
pub const UNIDIR_COMP_REFS: usize = 4; // BwdRefAltRef + 1

/// Total compound reference pairs.
pub const TOTAL_COMP_REFS: usize = FWD_REFS * BWD_REFS + TOTAL_UNIDIR_COMP_REFS;

/// Compound reference pairs (explicitly signaled).
pub const COMP_REFS: usize = FWD_REFS * BWD_REFS + UNIDIR_COMP_REFS;

/// Mode context reference frames count.
pub const MODE_CTX_REF_FRAMES: usize = TOTAL_REFS_PER_FRAME + TOTAL_COMP_REFS;

/// Reference mode for compound prediction signaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ReferenceMode {
    Single = 0,
    Compound = 1,
    Select = 2,
}

impl ReferenceMode {
    pub const COUNT: usize = 3;
}

/// Compound reference type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompReferenceType {
    Unidir = 0,
    Bidir = 1,
}

/// Reference list index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RefList {
    List0 = 0,
    List1 = 1,
}

pub const TOTAL_NUM_OF_REF_LISTS: usize = 2;

/// Prediction direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PredDirection {
    UniPredList0 = 0,
    UniPredList1 = 1,
    BiPred = 2,
}

/// Number of context values for various syntax elements.
pub const SKIP_CONTEXTS: usize = 3;
pub const SKIP_MODE_CONTEXTS: usize = 3;
pub const COMP_INDEX_CONTEXTS: usize = 6;
pub const COMP_GROUP_IDX_CONTEXTS: usize = 6;
pub const NEWMV_MODE_CONTEXTS: usize = 6;
pub const GLOBALMV_MODE_CONTEXTS: usize = 2;
pub const REFMV_MODE_CONTEXTS: usize = 6;
pub const DRL_MODE_CONTEXTS: usize = 3;
pub const INTER_MODE_CONTEXTS: usize = 8;
pub const INTRA_INTER_CONTEXTS: usize = 4;
pub const COMP_INTER_CONTEXTS: usize = 5;
pub const REF_CONTEXTS: usize = 3;
pub const COMP_REF_TYPE_CONTEXTS: usize = 5;
pub const UNI_COMP_REF_CONTEXTS: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_frame_discriminants() {
        assert_eq!(ReferenceFrame::None as i8, -1);
        assert_eq!(ReferenceFrame::Intra as i8, 0);
        assert_eq!(ReferenceFrame::Last as i8, 1);
        assert_eq!(ReferenceFrame::AltRef as i8, 7);
    }

    #[test]
    fn ref_frame_counts() {
        assert_eq!(INTER_REFS_PER_FRAME, 7);
        assert_eq!(TOTAL_REFS_PER_FRAME, 8);
        assert_eq!(TOTAL_COMP_REFS, 4 * 3 + 9);
        assert_eq!(MODE_CTX_REF_FRAMES, 8 + 21);
    }
}
