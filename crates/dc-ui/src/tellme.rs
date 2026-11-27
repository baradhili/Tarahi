//! The Tell Me command registry (#204 step 1): the ribbon tree
//! flattened into a searchable command list.
//!
//! `ribbon::groups(tab, ctx)` is a pure function of the [`TabCtx`]
//! snapshot and every `RibbonButton` already carries the (label,
//! tip, icon, enabled, action) the search needs — so the registry IS
//! the tree, flattened per frame from the same snapshot the ribbon
//! renders from (the keytips path already rebuilds the tree twice
//! per frame; flattening all tabs is the same cost). Zero drift by
//! construction: a command becomes searchable the moment its button
//! exists, in exactly the states the ribbon paints it.
//!
//! Result labels follow the DISPLAY language — [`ribbon::build`]
//! reads the active catalog, so entries carry the same localized
//! text their ribbon buttons paint (owner direction, 2026-11-27).
//! Matching by English words under a non-English catalog is step
//! 2's `anchor_labels`-style dual build, not this one.
//!
//! The tree is walked in its UNMIRRORED (LTR) order whatever the
//! display language: a search index is not a painted surface, so
//! rank ties keep one stable order across languages (the RTL mirror
//! in `groups` is a paint-time concern; the popup mirrors when it
//! paints, step 3).

use crate::ribbon::{self, Action, Col, Icon, RibbonButton, Tab, TabCtx};

/// One searchable command: a ribbon button flattened out of its tab's
/// group tree, plus the overlay data the tree can't derive (#204
/// step 1).
pub struct CommandEntry {
    /// The tab the button lives on — ContainerTools included: the
    /// spec's contextual-only commands stay findable while nothing
    /// selects a container, in their faded-gray unavailable state.
    pub tab: Tab,
    pub icon: Icon,
    /// The button's a11y label — the compact icon-only tier keeps
    /// its name in `alt` (#252). This is the text result rows show,
    /// in the display language.
    pub label: String,
    pub tip: String,
    /// Mirrors the button under the same `TabCtx` the ribbon renders
    /// from — the spec's unavailable state without a second context
    /// query (#204 step 5's "for free").
    pub enabled: bool,
    pub action: Action,
    /// Synonyms the label lacks (the overlay keyed by `Action`).
    pub keywords: &'static [&'static str],
    /// The command's keyboard accelerator where the app already binds
    /// one (`app/keys.rs`).
    pub shortcut: Option<&'static str>,
}

/// Flatten EVERY tab's tree — the fixed [`ribbon::TABS`] plus the
/// contextual ContainerTools — into the searchable command list, in
/// strip order. Pure function of the snapshot; callers rebuild per
/// frame, never cache.
pub fn registry(ctx: &TabCtx) -> Vec<CommandEntry> {
    let s = crate::i18n::strs();
    let mut out = Vec::new();
    for tab in ribbon::TABS.iter().copied().chain([Tab::ContainerTools]) {
        for g in ribbon::build(s, tab, ctx) {
            for col in g.cols {
                flatten_col(tab, col, &mut out);
            }
        }
    }
    out
}

fn flatten_col(tab: Tab, col: Col, out: &mut Vec<CommandEntry>) {
    match col {
        Col::Big(b) => out.push(entry(tab, b)),
        Col::Stack(v) => out.extend(v.into_iter().map(|b| entry(tab, b))),
        Col::Grid(rows) => {
            for row in rows {
                out.extend(row.into_iter().map(|b| entry(tab, b)));
            }
        }
    }
}

fn entry(tab: Tab, b: RibbonButton) -> CommandEntry {
    let (keywords, shortcut) = overlay(b.action);
    CommandEntry {
        tab,
        icon: b.icon,
        label: b.a11y_label().to_string(),
        tip: b.tip,
        enabled: b.enabled,
        action: b.action,
        keywords,
        shortcut,
    }
}

/// The keywords/shortcut overlay (#204 step 1): the small static
/// table the flattened tree cannot derive — synonyms a command's own
/// label lacks, and the accelerator the app already binds elsewhere.
/// Keyed by `Action`; a command whose label says it all carries
/// nothing.
///
/// Deliberately narrow:
/// - `Action::Macros` carries nothing — it is also the placeholder
///   action of every unwired ribbon stub, and synonyms would attach
///   to all of them at once.
/// - QAT-only commands (Save, Undo, Redo — Ctrl+S/Z/Y) carry no
///   shortcuts here: they are not tree commands, so the registry
///   never sees them (widening past the tree is not this step).
fn overlay(action: Action) -> (&'static [&'static str], Option<&'static str>) {
    const NONE: &[&str] = &[];
    match action {
        // The acceptance walk types "align": the six align commands
        // must surface, but "Center", "Middle" and "To Page" don't
        // contain the word.
        Action::AlignHCenter | Action::AlignVCenter | Action::AlignToPage => (&["align"], None),
        // "Dist H"/"Dist V" abbreviate Visio's "Distribute".
        Action::DistributeH | Action::DistributeV => (&["distribute"], None),
        // The spec's §6 example: typing "link" surfaces the link
        // commands; the label is "Hyperlink".
        Action::Hyperlink => (&["link"], None),
        // The Zoom group's level commands — "Whole Page", "Page
        // Width", "100%", "Selection" — none contains "zoom".
        Action::FitPage | Action::FitPageWidth | Action::Zoom100 => (&["zoom"], None),
        Action::ZoomSelection => (&["zoom"], Some("Shift+Z")),
        // Accelerators the app already binds (`app/keys.rs`).
        Action::SelectAll => (NONE, Some("Ctrl+A")),
        Action::SpellCheck => (NONE, Some("F7")),
        _ => (NONE, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tool;

    /// The sparsest snapshot the ribbon can paint: no document, no
    /// selection, nothing open.
    fn ctx() -> TabCtx {
        TabCtx {
            has_doc: false,
            has_sel: false,
            multi_sel: false,
            tri_sel: false,
            tool: Tool::Pointer,
            painter_armed: false,
            selby_open: false,
            align_to_page: false,
            pagesetup_open: false,
            backgrounds_open: false,
            contgal_open: false,
            show_rulers: false,
            show_grid: false,
            show_comments: false,
            size_pos_open: false,
            dynamic_grid: false,
            snap_dlg_open: false,
            autoconnect: false,
            chrome_hidden: false,
            shapesheet_open: false,
            page_sheet_open: false,
            doc_sheet_open: false,
            sel_group: false,
            sel_container: false,
            container_highlight: false,
        }
    }

    fn find(reg: &[CommandEntry], a: Action) -> &CommandEntry {
        reg.iter()
            .find(|e| e.action == a)
            .expect("action lives in the ribbon tree")
    }

    /// Every tab's tree is flattened, the contextual ContainerTools
    /// included — its commands stay findable (unavailable) even with
    /// nothing selected, per the acceptance criteria.
    #[test]
    fn spans_all_tabs_including_container_tools() {
        let reg = registry(&ctx());
        for tab in ribbon::TABS.iter().copied().chain([Tab::ContainerTools]) {
            if tab == Tab::File {
                continue; // the backstage view, no content tree
            }
            assert!(reg.iter().any(|e| e.tab == tab), "no entry from {tab:?}");
        }
        let disband = find(&reg, Action::ContainerDisband);
        assert_eq!(disband.tab, Tab::ContainerTools);
        assert!(!disband.enabled); // nothing selected → unavailable
    }

    /// Entries carry the a11y label, not the painted one — the
    /// compact icon-only tier paints "" and keeps its name in `alt`
    /// (#252); a registry row must never lose that name.
    #[test]
    fn entries_carry_a11y_labels() {
        let reg = registry(&ctx());
        assert!(reg.iter().all(|e| !e.label.is_empty()));
        // An icon-only command: Home's Lasso paints no label.
        assert!(!find(&reg, Action::LassoTool).label.is_empty());
    }

    /// `enabled` mirrors the same snapshot the ribbon renders from —
    /// the spec's state synchronization, by construction.
    #[test]
    fn enabled_mirrors_the_snapshot() {
        let sparse = registry(&ctx());
        assert!(!find(&sparse, Action::NewPage).enabled); // no document
        assert!(!find(&sparse, Action::Group).enabled); // no multi-selection
        let working = registry(&TabCtx {
            has_doc: true,
            has_sel: true,
            multi_sel: true,
            sel_container: true,
            ..ctx()
        });
        assert!(find(&working, Action::NewPage).enabled);
        assert!(find(&working, Action::Group).enabled);
        assert!(find(&working, Action::ContainerDisband).enabled);
    }

    /// The overlay: synonyms the labels lack, accelerators the app
    /// already binds — and nothing where the label says it all.
    #[test]
    fn overlay_carries_keywords_and_shortcuts() {
        let reg = registry(&ctx());
        assert!(find(&reg, Action::AlignHCenter).keywords.contains(&"align"));
        assert!(find(&reg, Action::AlignVCenter).keywords.contains(&"align"));
        assert!(find(&reg, Action::AlignToPage).keywords.contains(&"align"));
        assert!(find(&reg, Action::DistributeH).keywords.contains(&"distribute"));
        assert!(find(&reg, Action::Hyperlink).keywords.contains(&"link"));
        assert!(find(&reg, Action::FitPage).keywords.contains(&"zoom"));
        assert!(find(&reg, Action::Zoom100).keywords.contains(&"zoom"));
        assert_eq!(find(&reg, Action::ZoomSelection).shortcut, Some("Shift+Z"));
        assert!(find(&reg, Action::ZoomSelection).keywords.contains(&"zoom"));
        assert_eq!(find(&reg, Action::SelectAll).shortcut, Some("Ctrl+A"));
        assert_eq!(find(&reg, Action::SpellCheck).shortcut, Some("F7"));
        // A command whose label says it all: no overlay data.
        let grid = find(&reg, Action::ToggleGrid);
        assert!(grid.keywords.is_empty());
        assert_eq!(grid.shortcut, None);
    }

    /// Strip order, stable across calls — the rank ties of step 2's
    /// matcher keep one order whatever the display language (the
    /// unmirrored `build` walk, not `groups`'s RTL reversal).
    #[test]
    fn order_is_strip_order_and_stable() {
        let a = registry(&ctx());
        let b = registry(&ctx());
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| x.action == y.action));
        assert_eq!(a.first().map(|e| e.tab), Some(Tab::Home)); // File has no tree
        assert_eq!(a.last().map(|e| e.tab), Some(Tab::ContainerTools));
    }
}
