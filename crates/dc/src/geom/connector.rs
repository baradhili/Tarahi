//! The connector-identity checks: `ObjType` and the glue-signature
//! gate the hit walk and the crossing set share.

use vsdx::model::Shape;

/// `ObjType=2` — the routable connector marker (corpus Dynamic
/// connector).
pub fn is_connector(s: &Shape) -> bool {
    s.sheet
        .cell("ObjType")
        .and_then(|c| c.numeric())
        .is_some_and(|v| v == 2.0)
}

/// The chain-aware form (#172): a master-based connector instance
/// carries `ObjType=2` on its MASTER, not locally — the local-only
/// check missed every Visio-authored connector (endpoint drags fell
/// to the plain-1D path and were refused by the glue formulas).
///
/// #226 (MisterDiagram parity evidence, utilisation.vsdx): Visio also
/// authors 1D glue connectors WITHOUT ObjType — the corpus Line-arc
/// master marks itself with the glue engine's signature instead
/// (`GlueType` + `BegTrigger`/`EndTrigger`, no ObjType at all). A
/// resolved `BeginX` plus a glue trigger on either end is that
/// signature: the endpoint cells exist (XForm1D) and the glue engine
/// owns them. Plain 1D lines (the blank template's Line master) carry
/// no triggers and stay out — they are not glue shapes.
pub fn is_connector_in(doc: &vsdx::Document, chain: &[&Shape], s: &Shape) -> bool {
    if is_connector(s) {
        return true;
    }
    // The #174 chain-resolved form: a master-based connector carries
    // ObjType=2 on its MASTER.
    if doc
        .resolve_cell(s, chain, "ObjType")
        .and_then(|c| c.numeric())
        .is_some_and(|v| v == 2.0)
    {
        return true;
    }
    let resolved = |n: &str| doc.resolve_cell(s, chain, n);
    let one_d = resolved("BeginX").is_some();
    // A trigger is the glue engine's `_XFTRIGGER(…)` write (or a
    // nonzero value). The `V='0' F='No Formula'` every stylesheet
    // carries is no trigger: once style resolution reaches those
    // defaults, a presence test made every 1D line a connector (the
    // network LLD's Staging page went from 176 to 1361 jump hops).
    let trigger = |n: &str| {
        resolved(n).is_some_and(|c| {
            c.f.as_deref().is_some_and(|f| f.contains("_XFTRIGGER"))
                || c.numeric().is_some_and(|v| v != 0.0)
        })
    };
    let triggers = trigger("BegTrigger") || trigger("EndTrigger");
    // Glue-owned endpoints (#250): a STATIC glue write (`PAR(PNT(...))`)
    // leaves no ObjType and no trigger cells, but the formula is the
    // glue engine's own signature — user-authored connectors and
    // stencil masters statically glued in the app reroute through it
    // too. `_WALKGLUE` covers a dynamic end whose trigger write was
    // skipped.
    let glue_formula = |c: Option<&vsdx::Cell>| {
        c.and_then(|c| c.f.clone())
            .is_some_and(|f| f.contains("PAR(PNT(") || f.contains("_WALKGLUE("))
    };
    let glue_owned = triggers
        || glue_formula(resolved("BeginX").as_ref())
        || glue_formula(resolved("EndX").as_ref());
    one_d && glue_owned
}
