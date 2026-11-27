//! The Tell Me command registry and matcher (#204 steps 1–2): the
//! ribbon tree flattened into a searchable command list.
//!
//! `ribbon::groups(tab, ctx)` is a pure function of the [`TabCtx`]
//! snapshot and every `RibbonButton` already carries the (label,
//! tip, icon, enabled, action) the search needs — so the registry IS
//! the tree, flattened per frame from the same snapshot the ribbon
//! renders from. Zero drift by construction: a command becomes
//! searchable the moment its button exists, in exactly the states
//! the ribbon paints it.
//!
//! Result labels follow the DISPLAY language — [`ribbon::build`]
//! reads the active catalog, so entries carry the same localized
//! text their ribbon buttons paint (owner direction, 2026-11-27).
//! Each entry also carries its ENGLISH anchor label (the #257
//! dual-build trick `anchor_labels` uses: the tree is rebuilt under
//! `L10n::En` and flattened in the identical order — the builders
//! are pure functions of the snapshot, so the two runs are
//! structurally identical and the positional zip pairs each entry
//! with its own English name). Step 2's matcher ranks over label +
//! keywords + anchor, so a German or Persian chrome finds commands
//! in either language in the same run.
//!
//! The tree is walked in its UNMIRRORED (LTR) order whatever the
//! display language: a search index is not a painted surface, so
//! rank ties keep one stable order across languages (the RTL mirror
//! in `groups` is a paint-time concern; the popup mirrors when it
//! paints, step 3).

use crate::ribbon::{self, Action, Col, Icon, RibbonButton, Tab, TabCtx};

/// One searchable command: a ribbon button flattened out of its tab's
/// group tree, plus the overlay data the tree can't derive (#204
/// step 1). `Clone` since step 3: the panel owns its per-frame rows.
#[derive(Clone)]
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
    /// The ENGLISH a11y label (the #257 anchor): the text a
    /// non-English chrome matches by, so English words find
    /// commands under any catalog. Equals `label` under an English
    /// UI.
    pub anchor: String,
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
/// frame, never cache. Each tab's tree is built twice, once under
/// the active catalog (the display labels) and once under English
/// (the anchors) — the `anchor_labels` cost class.
pub fn registry(ctx: &TabCtx) -> Vec<CommandEntry> {
    let s = crate::i18n::strs();
    let en = crate::i18n::strs_for(crate::l10n::L10n::En);
    let mut out = Vec::new();
    for tab in ribbon::TABS.iter().copied().chain([Tab::ContainerTools]) {
        let act = buttons(ribbon::build(s, tab, ctx));
        let anchor = buttons(ribbon::build(en, tab, ctx));
        // Pure functions of the same snapshot: the two builds are
        // structurally identical (the #257 invariant anchor_labels
        // already rides), so the positional zip pairs each entry
        // with its OWN English name.
        debug_assert_eq!(act.len(), anchor.len(), "dual builds must match");
        for (b, eb) in act.into_iter().zip(anchor) {
            out.push(entry(tab, b, eb));
        }
    }
    out
}

/// Flatten a tab's group tree into its buttons, in content order
/// (group → column → stack/grid row → button).
fn buttons(groups: Vec<ribbon::Group>) -> Vec<RibbonButton> {
    let mut out = Vec::new();
    for g in groups {
        for col in g.cols {
            match col {
                Col::Big(b) => out.push(b),
                Col::Stack(v) => out.extend(v),
                Col::Grid(rows) => {
                    for row in rows {
                        out.extend(row);
                    }
                }
            }
        }
    }
    out
}

fn entry(tab: Tab, b: RibbonButton, en: RibbonButton) -> CommandEntry {
    let (keywords, shortcut) = overlay(b.action);
    CommandEntry {
        tab,
        icon: b.icon,
        label: b.a11y_label().to_string(),
        anchor: en.a11y_label().to_string(),
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
        // are the bare direction words "Left", "Center", "Right",
        // "Top", "Middle", "Bottom" (plus "To Page") — none contains
        // the word "align".
        Action::AlignLeft
        | Action::AlignRight
        | Action::AlignTop
        | Action::AlignBottom
        | Action::AlignHCenter
        | Action::AlignVCenter
        | Action::AlignToPage => (&["align"], None),
        // "Dist H"/"Dist V" abbreviate Visio's "Distribute" — and
        // nothing more (owner call, 2026-11-27: Distribute does not
        // match "align").
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

/// Match quality (#204 step 2): prefix beats word-start beats
/// substring. Ordinal order IS the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    /// The string starts with the token.
    Prefix,
    /// Some word inside the string starts with the token.
    WordStart,
    /// The token appears anywhere in the string.
    Substring,
}

/// Result-row cap (#204 step 2: "cap ~8").
const RESULT_CAP: usize = 8;

/// The query field's stable egui id (#204 step 4): the shell lays the
/// field out in the Tell Me box; Alt+Q and the Q keytip request focus
/// on it from the input path.
pub fn field_id() -> egui::Id {
    egui::Id::unique("tellme-field")
}

/// One panel row (#204 step 6): a search result, or an idle "Try"
/// suggestion shown while the query is empty (spec 3.1 — clicking /
/// Enter populates the query, a drill-down).
pub enum PanelRow {
    Result(CommandEntry),
    Try {
        /// The localized intent text the row shows.
        label: String,
        /// The ENGLISH drill string that populates the query — it
        /// matches through the English anchor under any display
        /// language.
        query: &'static str,
    },
}

/// The panel's rows for a query (#204 steps 2–6): the live search
/// results while typing; the idle Try list while the query is empty.
pub fn rows(ctx: &TabCtx, query: &str) -> Vec<PanelRow> {
    if query.trim().is_empty() {
        return try_rows();
    }
    search(&registry(ctx), query)
        .into_iter()
        .map(|e| PanelRow::Result(e.clone()))
        .collect()
}

/// The idle "Try" list (#204 step 6, spec 3.1): curated from REAL
/// delivered commands only — Visio's example intents (a theme, CAD
/// insert, link-shapes-to-data) map to areas we don't ship (the
/// Themes gallery is still a disabled stub; CAD/data-link are
/// undelivered). Each intent drills into a query that finds its
/// command.
fn try_rows() -> Vec<PanelRow> {
    let s = crate::i18n::strs();
    vec![
        PanelRow::Try {
            label: s.msg_try_page_orientation(),
            query: "orientation",
        },
        PanelRow::Try {
            label: s.msg_try_grid(),
            query: "grid",
        },
        PanelRow::Try {
            label: s.msg_try_whole_page(),
            query: "whole page zoom",
        },
        PanelRow::Try {
            label: s.msg_try_connectors(),
            query: "connector",
        },
    ]
}

/// Search the registry (#204 step 2). Every whitespace token of the
/// query must match — case-insensitively — somewhere in the entry's
/// searchable text: the display-language label, the English anchor,
/// or the overlay keywords; an entry's rank is its WEAKEST token,
/// so a query whose words all prefix-match outranks one where a
/// word only substring-matches. Within a rank, enabled commands
/// come before disabled ones (the spec's own captures, §4.2/§4.4,
/// paint the faded unavailable rows after the actionable ones) and
/// ties then keep registry order (stable). Empty/whitespace queries
/// match nothing — the idle "Try" list (step 6) is the caller's to
/// show.
pub fn search<'a>(reg: &'a [CommandEntry], query: &str) -> Vec<&'a CommandEntry> {
    let tokens: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(MatchRank, bool, &CommandEntry)> = reg
        .iter()
        .filter_map(|e| {
            let mut rank = MatchRank::Prefix;
            for t in &tokens {
                rank = rank.max(token_rank(e, t)?);
            }
            Some((rank, !e.enabled, e))
        })
        .collect();
    // Stable sort: equal (rank, availability) keeps registry order.
    hits.sort_by_key(|&(rank, disabled, _)| (rank, disabled));
    hits.into_iter().map(|(_, _, e)| e).take(RESULT_CAP).collect()
}

/// The best rank one token achieves against an entry's searchable
/// strings; `None` when the token matches none of them.
fn token_rank(e: &CommandEntry, token: &str) -> Option<MatchRank> {
    let strings = std::iter::once(e.label.as_str())
        .chain(std::iter::once(e.anchor.as_str()))
        .chain(e.keywords.iter().copied());
    let mut best = None;
    for s in strings {
        let l = s.to_lowercase();
        let r = if l.starts_with(token) {
            MatchRank::Prefix
        } else if words(&l).any(|w| w.starts_with(token)) {
            MatchRank::WordStart
        } else if l.contains(token) {
            MatchRank::Substring
        } else {
            continue;
        };
        if best.is_none_or(|b| r < b) {
            best = Some(r);
        }
    }
    best
}

/// The words of a (lowercased) string: runs of alphanumerics, so
/// "Dist H" and Persian space/ZWNJ-separated text both split right.
fn words(lowercased: &str) -> impl Iterator<Item = &str> {
    lowercased
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
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

    /// A hand-built entry — the matcher is pure text, so fixtures
    /// prove two-language behavior without touching the
    /// process-global language (that switch lives in tests/i18n.rs).
    fn fixture(
        action: Action,
        label: &str,
        anchor: &str,
        keywords: &'static [&'static str],
        enabled: bool,
    ) -> CommandEntry {
        CommandEntry {
            tab: Tab::Home,
            icon: Icon::Grid,
            label: label.to_string(),
            anchor: anchor.to_string(),
            tip: String::new(),
            enabled,
            action,
            keywords,
            shortcut: None,
        }
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
    /// (#252); a registry row must never lose that name. The anchor
    /// is the same tier's English name.
    #[test]
    fn entries_carry_a11y_labels() {
        let reg = registry(&ctx());
        assert!(reg.iter().all(|e| !e.label.is_empty()));
        assert!(reg.iter().all(|e| !e.anchor.is_empty()));
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
            tri_sel: true,
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
        // Owner call, 2026-11-27: Distribute does NOT match "align".
        assert!(!find(&reg, Action::DistributeH).keywords.contains(&"align"));
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

    /// Prefix > word-start > substring, case-insensitively — and the
    /// rank beats registry order (the fixtures are inserted worst
    /// first).
    #[test]
    fn ranks_prefix_word_start_substring() {
        let reg = vec![
            fixture(Action::Macros, "Separate", "Separate", &[], true), // substring
            fixture(Action::Macros, "Format Painter", "Format Painter", &[], true), // word-start
            fixture(Action::Macros, "Paste", "Paste", &[], true),     // prefix
        ];
        let out = search(&reg, "pA");
        assert_eq!(
            out.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["Paste", "Format Painter", "Separate"]
        );
    }

    /// Two-language matching in one run: the display label, the
    /// English anchor, and the keywords are all searchable — a
    /// German chrome finds its command by any of the three.
    #[test]
    fn matches_in_both_languages_and_keywords() {
        let reg = vec![fixture(
            Action::Hyperlink,
            "Verknüpfung",
            "Verweis",
            &["link"],
            true,
        )];
        assert_eq!(search(&reg, "verkn").len(), 1); // display label
        assert_eq!(search(&reg, "verw").len(), 1); // English anchor
        assert_eq!(search(&reg, "link").len(), 1); // keyword
        assert_eq!(search(&reg, "LINK").len(), 1); // case-insensitive
        assert!(search(&reg, "hyperlink").is_empty()); // in neither string here
    }

    /// Every token must match, and the WEAKEST token governs the
    /// rank: "align le" puts a keyword-prefix pair above an earlier
    /// entry whose second token only word-starts.
    #[test]
    fn every_token_must_match_and_weakest_governs() {
        let reg = vec![
            fixture(Action::AlignLeft, "Align Left", "Align Left", &[], true),
            fixture(Action::Macros, "Align", "Align", &["left"], true),
        ];
        let out = search(&reg, "align le");
        assert_eq!(
            out.iter().map(|e| e.action).collect::<Vec<_>>(),
            [Action::Macros, Action::AlignLeft]
        );
        // A token nothing matches excludes the entry outright.
        assert!(search(&reg, "align zzz").is_empty());
    }

    /// Within a rank, enabled commands come before disabled ones
    /// (spec §4.2/§4.4 paint the faded rows after the actionable
    /// ones) — but a prefix match on a disabled command still beats
    /// a substring match on an enabled one (relevance first).
    #[test]
    fn enabled_ranks_above_disabled_within_a_tier() {
        let reg = vec![
            fixture(Action::Macros, "Paste", "Paste", &[], false), // disabled prefix
            fixture(Action::Macros, "Paste Special", "Paste Special", &[], true),
            fixture(Action::Macros, "Separate", "Separate", &[], true), // substring
        ];
        let out = search(&reg, "pa");
        assert_eq!(
            out.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["Paste Special", "Paste", "Separate"]
        );
    }

    /// The cap: at most RESULT_CAP rows, stable registry order
    /// among ties.
    #[test]
    fn caps_results_and_keeps_stable_order() {
        let reg: Vec<CommandEntry> = ["za", "zb", "zc", "zd", "ze", "zf", "zg", "zh", "zi", "zj"]
            .iter()
            .map(|l| fixture(Action::Macros, l, l, &[], true))
            .collect();
        let out = search(&reg, "z");
        assert_eq!(out.len(), RESULT_CAP);
        assert_eq!(
            out.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["za", "zb", "zc", "zd", "ze", "zf", "zg", "zh"]
        );
    }

    /// Empty and whitespace-only queries match nothing — the idle
    /// "Try" list is the caller's (step 6).
    #[test]
    fn empty_queries_match_nothing() {
        let reg = registry(&ctx());
        assert!(search(&reg, "").is_empty());
        assert!(search(&reg, "   ").is_empty());
    }

    /// The acceptance walk against the real tree: typing "align"
    /// (with a multi-selection live, so the commands are usable)
    /// surfaces To Page and the six align commands — the bare
    /// direction-word labels match via their keyword — with the
    /// first disabled Home text-align stub taking the last row
    /// under the cap. Distribute does not match "align" (owner
    /// call, 2026-11-27); "distribute" is its query.
    #[test]
    fn align_walk_surfaces_the_align_commands() {
        let working = TabCtx {
            has_doc: true,
            has_sel: true,
            multi_sel: true,
            tri_sel: true,
            ..ctx()
        };
        let reg = registry(&working);
        let out = search(&reg, "align");
        assert_eq!(
            out.iter().map(|e| e.action).collect::<Vec<_>>(),
            [
                Action::AlignToPage,   // "To Page" via keyword
                Action::AlignLeft,     // "Left" via keyword
                Action::AlignHCenter,  // "Center" via keyword
                Action::AlignRight,    // "Right" via keyword
                Action::AlignTop,      // "Top" via keyword
                Action::AlignVCenter,  // "Middle" via keyword
                Action::AlignBottom,   // "Bottom" via keyword
                Action::Macros,        // the disabled "Align Left" text stub
            ]
        );
        assert!(!out[7].enabled); // the stub renders faded
        let dist = search(&reg, "distribute");
        assert_eq!(
            dist.iter().map(|e| e.action).collect::<Vec<_>>(),
            [Action::DistributeH, Action::DistributeV]
        );
    }

    /// The contextual-only acceptance: Container Tools commands are
    /// findable while nothing selects a container — rendered
    /// unavailable, still returned.
    #[test]
    fn container_tools_findable_while_unselected() {
        let reg = registry(&ctx()); // nothing selected
        let out = search(&reg, "disband");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].action, Action::ContainerDisband);
        assert!(!out[0].enabled);
    }
}
