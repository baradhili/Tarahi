//! Verbatim-kept elements and ordered attribute lists.

/// Ordered attribute list, in document order (round-trip fidelity).
pub type Attrs = Vec<(String, String)>;

/// An element the model does not interpret (`Icon`, `FaceNames`,
/// `DocumentSettings`, …), kept verbatim: its opening-tag attributes plus
/// the raw inner XML, re-emitted unchanged on write.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawEl {
    pub name: String,
    pub attrs: Attrs,
    /// Inner XML exactly as read (children, text, entities, CDATA).
    pub inner: String,
}
