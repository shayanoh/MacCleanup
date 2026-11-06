/// How a confirmed deletion proceeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the ToBeDeleted folder only.
    SoftOnly,
    /// Move, then permanently empty what was just moved.
    SoftThenEmpty,
}
