//! Evaluation environments for instance-time formula re-evaluation.

use crate::formula;

use super::cell::Cell;
use super::shape::Shape;

/// Evaluation environment for re-evaluating a master root's cells at
/// instance size — shared by the SVG renderer's master mirroring
/// (issue #15) and the editor's flattened-group cache refresh
/// (issue #59), so both compute placements identically. Seeds the
/// group's effective `Width`/`Height` under the bare and `Sheet.N!`
/// spellings master formulas use, plus the root's User cells — their
/// formulas re-evaluate too, because layout cells like
/// `User.IconY = Height - User.InfoPanelHeight/2` reference the
/// (instance) `Height`. Instance-local User rows override the master's
/// (an instance can change editable rows such as `ShapeLayout`), and
/// two passes let a User formula reference another User formula.
pub fn instance_env(
    master_root: &Shape,
    instance: &Shape,
    width: f64,
    height: f64,
) -> formula::Env {
    let mut env = formula::Env::new();
    env.insert("Width".into(), width);
    env.insert("Height".into(), height);
    env.insert(format!("Sheet.{}!Width", master_root.id), width);
    env.insert(format!("Sheet.{}!Height", master_root.id), height);
    let mut users: Vec<(String, Cell)> = Vec::new();
    for user in master_root.sheet.sections_named("User") {
        for row in user.rows.iter().filter(|r| r.n.is_some()) {
            if let Some(c) = row.cell("Value") {
                users.push((row.n.clone().unwrap(), c.clone()));
            }
        }
    }
    for user in instance.sheet.sections_named("User") {
        for row in user.rows.iter().filter(|r| r.n.is_some()) {
            if let Some(c) = row.cell("Value") {
                // An instance row cached under F='Inh' is a frozen
                // snapshot of the master's formula — the master row (and
                // its formula, re-evaluated below) still owns the value.
                // Only a genuine local override (V with no F, or a real
                // formula — e.g. an instance's ShapeLayout=4) replaces
                // the master's row. Treating caches as overrides would
                // freeze layout cells like User.IconY at their save-time
                // value when re-evaluating after an edit (issue #59).
                if c.is_inherited() {
                    continue;
                }
                if let Some(slot) = users
                    .iter_mut()
                    .find(|(n, _)| n == row.n.as_deref().unwrap())
                {
                    slot.1 = c.clone();
                } else {
                    users.push((row.n.clone().unwrap(), c.clone()));
                }
            }
        }
    }
    let set = |env: &mut formula::Env, name: &str, v: f64| {
        env.insert(format!("User.{name}"), v);
        env.insert(format!("Sheet.{}!User.{name}", master_root.id), v);
    };
    for (name, c) in &users {
        if let Some(v) = c.numeric().filter(|v| v.is_finite()) {
            set(&mut env, name, v);
        }
    }
    for _ in 0..2 {
        let mut changed = false;
        for (name, c) in &users {
            let Some(f) = c.f.as_deref() else { continue };
            if f == "Inh" || f == "No Formula" {
                continue;
            }
            if let Some(v) = formula::eval(f, &env)
                .ok()
                .and_then(|v| v.as_f64())
                .filter(|v| v.is_finite())
            {
                let cur = env.get(&format!("User.{name}")).copied();
                if cur != Some(v) {
                    set(&mut env, name, v);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    env
}

/// A sub-shape's evaluation environment base: only QUALIFIED root
/// entries (`Sheet.N!…`) carry over — a bare name in a child formula
/// means the child's own cell, so the root's unqualified Width/Height
/// and User.* must not leak in. The caller then seeds the child's own
/// effective `Width`/`Height` (child sheets are not in the root env;
/// bare child-cell references fail to resolve and fall back).
pub fn qualified_env(root_env: &formula::Env) -> formula::Env {
    root_env
        .iter()
        .filter(|(k, _)| k.contains('!'))
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}
