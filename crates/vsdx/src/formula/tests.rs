use super::*;

fn env() -> Env {
    let mut e = Env::new();
    e.insert("Width".into(), 2.0);
    e.insert("Height".into(), 1.0);
    e.insert("Geometry1.X1".into(), 0.5);
    e
}

#[test]
fn arithmetic_and_refs() {
    assert_eq!(eval("Width*0.5", &env()).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("Width/Height*1", &env()).unwrap().as_f64(), Some(2.0));
    assert_eq!(eval("Geometry1.X1+1", &env()).unwrap().as_f64(), Some(1.5));
    assert_eq!(eval("-Width+5", &env()).unwrap().as_f64(), Some(3.0));
    assert_eq!(eval("2^3", &env()).unwrap().as_f64(), Some(8.0));
}

#[test]
fn strings() {
    let v = eval("\"R\"&1", &env()).unwrap();
    assert_eq!(v, Value::Str("R1".into()));
}

#[test]
fn unit_literals_convert_to_internal_units() {
    assert_eq!(eval("0.24PT", &env()).unwrap().as_f64(), Some(0.24 / 72.0));
    assert_eq!(eval("1PT+1PT", &env()).unwrap().as_f64(), Some(2.0 / 72.0));
    assert_eq!(
        eval("90DEG", &env()).unwrap().as_f64(),
        Some(90.0 * std::f64::consts::PI / 180.0)
    );
    let mm = eval("25.4MM", &env()).unwrap().as_f64().unwrap();
    assert!((mm - 1.0).abs() < 1e-12);
    // Unit-less (DL/local) literals are unchanged.
    assert_eq!(eval("Width-0.5DL", &env()).unwrap().as_f64(), Some(1.5));
}

#[test]
fn scientific_notation_literals() {
    // The Role-callout specimen (issue #15): `Height*9.2184490821108E-5`
    // misparsed as `Height*9.2184-5` and threw an arc control point
    // four inches below a 0.1-inch shape.
    let e = env(); // Height = 1
    assert_eq!(
        eval("Height*9.2184490821108E-5", &e).unwrap().as_f64(),
        Some(9.2184490821108E-5)
    );
    assert_eq!(eval("1E2", &e).unwrap().as_f64(), Some(100.0));
    assert_eq!(eval("2.5e+3", &e).unwrap().as_f64(), Some(2500.0));
    assert_eq!(eval("1E-2PT", &e).unwrap().as_f64(), Some(0.01 / 72.0));
    // A trailing `E` that is not an exponent still falls back to the
    // (unknown → 1.0) unit factor rather than breaking the number.
    assert_eq!(eval("1.5", &e).unwrap().as_f64(), Some(1.5));
}

#[test]
fn unsupported_falls_back_to_err() {
    assert!(eval("THEMEVAL(\"LineWeight\",0.24PT)", &env()).is_err());
    assert!(eval("Inh", &env()).is_err());
    // Without a metrics bridge the text functions stay unsupported
    // (the pre-#147 contract: callers fall back to the cached V).
    assert!(eval("TEXTWIDTH(TheText)", &env()).is_err());
    assert!(eval("TEXTHEIGHT(TheText,TxtWidth)", &env()).is_err());
}

/// A deterministic fake bridge: width = 0.1×chars, height = the
/// width passed in (so tests can see which width the function
/// forwarded).
struct FakeText;

impl TextMeasure for FakeText {
    fn the_text(&self) -> Option<String> {
        Some("hello\r\n".into())
    }
    fn text_width(&self, text: &str) -> Option<f64> {
        Some(0.1 * text.trim_end().chars().count() as f64)
    }
    fn text_height(&self, _text: &str, width: f64) -> Option<f64> {
        Some(width * 2.0)
    }
}

#[test]
fn text_functions_evaluate_through_a_bridge() {
    let e = env();
    let m = FakeText;
    // TheText arrives as the sheet's string; TEXTWIDTH measures it.
    assert_eq!(
        eval_measured("TEXTWIDTH(TheText)", &e, &m)
            .unwrap()
            .as_f64(),
        Some(0.5)
    );
    // String literals measure too, and arithmetic composes — the
    // corpus's `MAX(Width,TEXTWIDTH(TheText))` shapes evaluate
    // end to end.
    assert_eq!(
        eval_measured("TEXTWIDTH(\"abc\")+1", &e, &m)
            .unwrap()
            .as_f64(),
        Some(1.3)
    );
    let mut e2 = env();
    e2.insert("Width".into(), 0.75);
    assert_eq!(
        eval_measured("MAX(Width, TEXTWIDTH(TheText))", &e2, &m)
            .unwrap()
            .as_f64(),
        Some(0.75)
    );
    assert_eq!(
        eval_measured("MIN(Width, TEXTWIDTH(TheText))", &e2, &m)
            .unwrap()
            .as_f64(),
        Some(0.5)
    );
    // Case-insensitive dispatch selects the right function
    // (CodeRabbit: `max(...)` used to take the MIN branch).
    assert_eq!(eval("max(1, 2)", &e2).unwrap().as_f64(), Some(2.0));
    assert_eq!(eval("Min(3, 2)", &e2).unwrap().as_f64(), Some(2.0));
    // TEXTHEIGHT forwards the resolved width argument.
    assert_eq!(
        eval_measured("TEXTHEIGHT(TheText, 0.25)", &e, &m)
            .unwrap()
            .as_f64(),
        Some(0.5)
    );
    // GUARD composes transparently over the text functions.
    assert_eq!(
        eval_measured("GUARD(TEXTHEIGHT(TheText, 0.25))", &e, &m)
            .unwrap()
            .as_f64(),
        Some(0.5)
    );
    // Arity errors are real errors, not cached-V fallbacks.
    assert!(eval_measured("TEXTHEIGHT(TheText)", &e, &m).is_err());
    assert!(eval_measured("TEXTWIDTH()", &e, &m).is_err());
}

#[test]
fn the_text_alone_resolves_as_a_string_under_a_bridge() {
    let e = env();
    let m = FakeText;
    let v = eval_measured("TheText&\"!\"", &e, &m).unwrap();
    assert_eq!(v, Value::Str("hello\r\n!".into()));
}

#[test]
fn comparisons_yield_one_or_zero() {
    let e = env();
    assert_eq!(eval("Width=2", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("Width<>2", &e).unwrap().as_f64(), Some(0.0));
    assert_eq!(eval("Height<Width", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("Width<=2", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("Width>2", &e).unwrap().as_f64(), Some(0.0));
    assert_eq!(eval("Width>=Height*2", &e).unwrap().as_f64(), Some(1.0));
}

#[test]
fn control_functions() {
    let e = env();
    assert_eq!(
        eval("IF(Width=2, 10, 20)", &e).unwrap().as_f64(),
        Some(10.0)
    );
    assert_eq!(
        eval("IF(Width=3, 10, 20)", &e).unwrap().as_f64(),
        Some(20.0)
    );
    assert_eq!(
        eval("OR(Width=3, Height=1)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    assert_eq!(
        eval("OR(Width=3, Height=2)", &e).unwrap().as_f64(),
        Some(0.0)
    );
    assert_eq!(
        eval("AND(Width=2, Height=1)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    assert_eq!(eval("NOT(Width=3)", &e).unwrap().as_f64(), Some(1.0));
    // GUARD transparently yields its protected expression.
    assert_eq!(
        eval("GUARD(4MM)", &e).unwrap().as_f64(),
        Some(4.0 * 0.03937007874015748)
    );
    assert_eq!(
        eval("THEMEGUARD(Width*0.5)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    // Unknown functions still error (callers fall back to cached V).
    assert!(eval("SETF(GetRef(Width), 1)", &e).is_err());
}

#[test]
fn sqrt_serves_the_chord_masters() {
    let mut e = env();
    e.insert("BeginX".into(), 1.0);
    e.insert("BeginY".into(), 6.0);
    e.insert("EndX".into(), 3.5);
    e.insert("EndY".into(), 6.2);
    // The Line-arc corpus master's Width formula: the endpoint
    // chord, not the signed dx.
    assert_eq!(
        eval("SQRT((EndX-BeginX)^2+(EndY-BeginY)^2)", &e)
            .unwrap()
            .as_f64(),
        Some(2.5f64.hypot(0.2))
    );
    assert_eq!(eval("SQRT(0)", &e).unwrap().as_f64(), Some(0.0));
    // Negative argument errors (callers fall back to cached V).
    assert!(eval("SQRT(Width-3)", &e).is_err());
}

#[test]
fn cross_sheet_references_resolve_from_env() {
    // The Role-callout specimen (issue #15): a child stretched with its
    // group evaluates `Sheet.5!Width*1` against the instance width.
    let mut e = env();
    e.insert("Sheet.5!Width".into(), 2.657480296178819);
    e.insert("Sheet.5!User.InfoPanelHeight".into(), 0.2362204724409449);
    assert_eq!(
        eval("Sheet.5!Width*1", &e).unwrap().as_f64(),
        Some(2.657480296178819)
    );
    assert_eq!(
        eval("GUARD(Sheet.5!User.InfoPanelHeight)", &e)
            .unwrap()
            .as_f64(),
        Some(0.2362204724409449)
    );
}

// ---- #122/FS-033 breadth slice ----

#[test]
fn math_functions_and_corpus_specimens() {
    let e = env();
    // The chord master's Angle (utilisation master3): y first.
    let mut e2 = env();
    e2.insert("EndX".into(), 3.5);
    e2.insert("EndY".into(), 6.2);
    e2.insert("BeginX".into(), 1.0);
    e2.insert("BeginY".into(), 6.0);
    assert_eq!(
        eval("ATAN2(EndY-BeginY,EndX-BeginX)", &e2)
            .unwrap()
            .as_f64(),
        Some((6.2f64 - 6.0).atan2(3.5 - 1.0))
    );
    // ATAN2(0,0) is 0, not a domain error.
    assert_eq!(eval("ATAN2(0,0)", &e).unwrap().as_f64(), Some(0.0));
    // ABS(User.Inset) (master16) composes with the control flow.
    let mut e3 = env();
    e3.insert("User.Inset".into(), -0.25);
    assert_eq!(eval("ABS(User.Inset)", &e3).unwrap().as_f64(), Some(0.25));
    // CEILING(TEXTHEIGHT(TheText,TxtWidth),margin) (master36).
    let m = FakeText;
    // TEXTHEIGHT = 0.52 → up to the 0.1 grid's 0.6.
    let ceiled = eval_measured("CEILING(TEXTHEIGHT(TheText,0.26),0.1)", &e, &m)
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(
        (ceiled - 0.6).abs() < 1e-9,
        "ceiling to the 0.1 grid: {ceiled}"
    );
    // The rest of the set at known values.
    assert_eq!(eval("INT(3.9)", &e).unwrap().as_f64(), Some(3.0));
    assert_eq!(eval("INT(-3.1)", &e).unwrap().as_f64(), Some(-4.0));
    assert_eq!(eval("SIGN(-9)", &e).unwrap().as_f64(), Some(-1.0));
    assert_eq!(eval("SIGN(0)", &e).unwrap().as_f64(), Some(0.0));
    assert_eq!(eval("EXP(0)", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("LN(EXP(2))", &e).unwrap().as_f64(), Some(2.0));
    let log1000 = eval("LOG(1000)", &e).unwrap().as_f64().unwrap();
    assert!((log1000 - 3.0).abs() < 1e-9, "LOG(1000) = 3: {log1000}");
    let log8 = eval("LOG(8,2)", &e).unwrap().as_f64().unwrap();
    assert!((log8 - 3.0).abs() < 1e-9, "LOG(8,2) = 3: {log8}");
    assert_eq!(
        eval("PI()", &e).unwrap().as_f64(),
        Some(std::f64::consts::PI)
    );
    assert_eq!(eval("DEG(PI())", &e).unwrap().as_f64(), Some(180.0));
    assert!(
        (eval("RAD(90)", &e).unwrap().as_f64().unwrap() - std::f64::consts::FRAC_PI_2).abs()
            < 1e-12
    );
    assert_eq!(eval("COS(0)", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("SIN(PI()/2)", &e).unwrap().as_f64(), Some(1.0));
    // Arity and domain errors stay errors (cached-V fallback).
    assert!(eval("ABS(1,2)", &e).is_err());
    assert!(eval("LN(0)", &e).is_err());
    assert!(eval("ACOS(2)", &e).is_err());
    assert!(eval("LOG(8,1)", &e).is_err());
    assert!(eval("PI(1)", &e).is_err());
}

#[test]
fn rounding_family_semantics() {
    let e = env();
    // CEILING away from zero on a multiple grid; FLOOR toward zero.
    assert_eq!(eval("CEILING(2.5,1)", &e).unwrap().as_f64(), Some(3.0));
    // The multiple must share the number's sign (else #NUM!).
    assert_eq!(eval("CEILING(-2.5,-1)", &e).unwrap().as_f64(), Some(-3.0));
    assert_eq!(eval("FLOOR(2.5,1)", &e).unwrap().as_f64(), Some(2.0));
    assert_eq!(eval("FLOOR(-2.5,-1)", &e).unwrap().as_f64(), Some(-2.0));
    // Non-1 multiples, sign matching, sign mismatch errors.
    assert_eq!(eval("CEILING(2.1,0.5)", &e).unwrap().as_f64(), Some(2.5));
    assert_eq!(eval("FLOOR(2.9,0.5)", &e).unwrap().as_f64(), Some(2.5));
    assert_eq!(eval("CEILING(2.5)", &e).unwrap().as_f64(), Some(3.0));
    assert_eq!(eval("CEILING(0,1)", &e).unwrap().as_f64(), Some(0.0));
    assert!(eval("CEILING(2, -1)", &e).is_err());
    // ROUND at a decimal precision, negative precision left of the
    // point; ties away from zero.
    assert_eq!(eval("ROUND(2.5)", &e).unwrap().as_f64(), Some(3.0));
    assert_eq!(eval("ROUND(-2.5)", &e).unwrap().as_f64(), Some(-3.0));
    assert_eq!(eval("ROUND(2.345,2)", &e).unwrap().as_f64(), Some(2.35));
    assert_eq!(eval("ROUND(1234,-2)", &e).unwrap().as_f64(), Some(1200.0));
    // Visio's MOD keeps the magnitude (the ADS_M Scratch.A1 rule).
    assert_eq!(eval("MOD(-5,2)", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("MOD(5,2)", &e).unwrap().as_f64(), Some(1.0));
    assert!(eval("MOD(5,0)", &e).is_err());
}

#[test]
fn string_functions() {
    let e = env();
    // The corpus shape: LEN(User.visCustomLabel)=0 (master10) —
    // an empty custom label tests as zero length.
    assert_eq!(eval("LEN(\"\")=0", &e).unwrap().as_f64(), Some(1.0));
    assert_eq!(eval("LEN(\"abc\")", &e).unwrap().as_f64(), Some(3.0));
    assert_eq!(
        eval("LEFT(\"abcdef\",2)", &e).unwrap(),
        Value::Str("ab".into())
    );
    assert_eq!(
        eval("RIGHT(\"abcdef\",2)", &e).unwrap(),
        Value::Str("ef".into())
    );
    // Over-long counts clamp to the text.
    assert_eq!(eval("LEFT(\"ab\",9)", &e).unwrap(), Value::Str("ab".into()));
    // 1-based MID.
    assert_eq!(
        eval("MID(\"abcdef\",2,3)", &e).unwrap(),
        Value::Str("bcd".into())
    );
    assert_eq!(eval("MID(\"abc\",9,2)", &e).unwrap(), Value::Str("".into()));
    // TRIM: single spaces between words, none at the edges.
    assert_eq!(
        eval("TRIM(\"  a   b  \")", &e).unwrap(),
        Value::Str("a b".into())
    );
    assert_eq!(
        eval("UPPER(\"aBc\")&LOWER(\"DeF\")", &e).unwrap(),
        Value::Str("ABCdef".into())
    );
    assert_eq!(
        eval("SUBSTITUTE(\"a-b-c\",\"-\",\"+\")", &e).unwrap(),
        Value::Str("a+b+c".into())
    );
    // FIND: 1-based position, optional 1-based start, case folding.
    assert_eq!(eval("FIND(\"b\",\"abc\")", &e).unwrap().as_f64(), Some(2.0));
    assert_eq!(
        eval("FIND(\"b\",\"abcabc\",3)", &e).unwrap().as_f64(),
        Some(5.0)
    );
    assert_eq!(
        eval("FIND(\"B\",\"abc\",1,1)", &e).unwrap().as_f64(),
        Some(2.0)
    );
    // Not-found and out-of-range starts are errors (cached-V fallback).
    assert!(eval("FIND(\"z\",\"abc\")", &e).is_err());
    assert!(eval("FIND(\"b\",\"abc\",9)", &e).is_err());
    // A needle longer than the haystack is not found — no panic
    // (#218 review).
    assert!(eval("FIND(\"abcd\",\"ab\")", &e).is_err());
    // VALUE parses a number out of text; garbage errors.
    assert_eq!(eval("VALUE(\"2.5\")", &e).unwrap().as_f64(), Some(2.5));
    assert!(eval("VALUE(\"abc\")", &e).is_err());
    assert!(eval("LEN(\"a\",\"b\")", &e).is_err());
}

#[test]
fn color_functions_match_the_measured_grid() {
    let e = env();
    // The legacy 240-step integer grid witness: HSL(144,116,125)
    // = #4979C0 (shipped-stencil measurement).
    assert_eq!(
        eval("HSL(144,116,125)", &e).unwrap().as_f64(),
        Some(0x4979C0 as f64)
    );
    // Full-saturation blue at luminosity 120: HSL(160,240,120) =
    // RGB(0,0,255).
    assert_eq!(
        eval("HSL(160,240,120)", &e).unwrap().as_f64(),
        eval("RGB(0,0,255)", &e).unwrap().as_f64()
    );
    assert_eq!(eval("RGB(0,0,255)", &e).unwrap().as_f64(), Some(255.0));
    // Component extraction is self-consistent.
    assert_eq!(eval("RED(RGB(10,20,30))", &e).unwrap().as_f64(), Some(10.0));
    assert_eq!(
        eval("GREEN(RGB(10,20,30))", &e).unwrap().as_f64(),
        Some(20.0)
    );
    assert_eq!(
        eval("BLUE(RGB(10,20,30))", &e).unwrap().as_f64(),
        Some(30.0)
    );
    // The fixed [MS-VSDX] palette: index 0 black, 1 white.
    assert_eq!(eval("RED(0)", &e).unwrap().as_f64(), Some(0.0));
    assert_eq!(eval("LUM(1)", &e).unwrap().as_f64(), Some(240.0));
    assert_eq!(eval("RED(1)", &e).unwrap().as_f64(), Some(255.0));
    // HUE/SAT/LUM on the 240 scale (green sits at hue 80).
    assert_eq!(eval("HUE(RGB(0,255,0))", &e).unwrap().as_f64(), Some(80.0));
    assert_eq!(eval("SAT(RGB(0,255,0))", &e).unwrap().as_f64(), Some(240.0));
    assert_eq!(eval("LUM(RGB(0,255,0))", &e).unwrap().as_f64(), Some(120.0));
    // Round-trip through the integer grid.
    assert_eq!(eval("LUM(HSL(0,0,120))", &e).unwrap().as_f64(), Some(120.0));
    // The measured TINT endpoint rule: TINT(RGB(225,225,225),28) =
    // RGB(233,233,233).
    assert_eq!(
        eval("TINT(RGB(225,225,225),28)", &e).unwrap().as_f64(),
        eval("RGB(233,233,233)", &e).unwrap().as_f64()
    );
    // The parity witnesses (LUM of the adjusted color).
    assert_eq!(
        eval("LUM(TINT(HSL(0,0,99),-1.5))", &e).unwrap().as_f64(),
        Some(99.0)
    );
    assert_eq!(
        eval("LUM(TINT(HSL(0,0,100),1.5))", &e).unwrap().as_f64(),
        Some(101.0)
    );
    assert_eq!(
        eval("LUM(TINT(HSL(0,0,211),100))", &e).unwrap().as_f64(),
        Some(240.0)
    );
    assert_eq!(
        eval("LUM(SHADE(HSL(0,0,99),1.5))", &e).unwrap().as_f64(),
        Some(99.0)
    );
    assert_eq!(
        eval("LUM(SHADE(HSL(0,0,141),-1.5))", &e).unwrap().as_f64(),
        Some(141.0)
    );
    // SHADE moves toward black; the corpus's SHADE(BaseColor,40)
    // darkens.
    let shaded = eval("LUM(SHADE(RGB(128,128,128),40))", &e)
        .unwrap()
        .as_f64()
        .unwrap();
    assert!(shaded < 128.0, "SHADE darkens: {shaded}");
    // Out-of-range and invalid inputs error (cached-V fallback).
    assert!(eval("HSL(241,0,0)", &e).is_err());
    assert!(eval("HSL(0,-1,0)", &e).is_err());
    assert!(eval("TINT(-3,10)", &e).is_err());
    // An invalid color extracts as 0 (the HUE/SAT/LUM rule).
    assert_eq!(eval("HUE(-3)", &e).unwrap().as_f64(), Some(0.0));
}

#[test]
fn guide_algebra_intersections() {
    let e = env();
    // A vertical guide at x=2 crossing a horizontal guide at y=3
    // (the guide-alignment cell pattern).
    let f = "INTERSECTX(2,0,2,1, 0,3,1,3)";
    assert_eq!(eval(f, &e).unwrap().as_f64(), Some(2.0));
    let fy = "INTERSECTY(2,0,2,1, 0,3,1,3)";
    assert_eq!(eval(fy, &e).unwrap().as_f64(), Some(3.0));
    // Off-axis crossing: y = x line vs y = 1 line → (1, 1).
    assert_eq!(
        eval("INTERSECTX(0,0,2,2, 0,1,2,1)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    assert_eq!(
        eval("INTERSECTY(0,0,2,2, 0,1,2,1)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    // Parallel lines error (cached-V fallback).
    assert!(eval("INTERSECTX(0,0,1,0, 2,0,3,0)", &e).is_err());
    assert!(eval("INTERSECTX(0,0,1,0, 0,1,1,1)", &e).is_err());
    assert!(eval("INTERSECTY(0,0,0,1, 1,0,1,1)", &e).is_err());
}

#[test]
fn themeguard_identity_and_isthemed_false() {
    let e = env();
    // THEMEGUARD guards the cell from theme overwrites; this
    // evaluator applies no themes, so the identity IS the
    // guarded evaluation.
    assert_eq!(
        eval("THEMEGUARD(Width*0.5)", &e).unwrap().as_f64(),
        Some(1.0)
    );
    // No theme engine: the measured no-theme branch is false.
    assert_eq!(eval("ISTHEMED()", &e).unwrap().as_f64(), Some(0.0));
    assert!(eval("ISTHEMED(1)", &e).is_err());
}

#[test]
fn role_icon_y_specimen() {
    // User.IconY from master5.xml, verbatim shape:
    // IF(OR(User.ShapeLayout<>1), Height-User.InfoPanelHeight/2, User.InfoPanelHeight/2)
    let mut e = env();
    e.insert("User.ShapeLayout".into(), 0.0);
    e.insert("User.InfoPanelHeight".into(), 0.2362204724409449);
    // Height=1 from env(): 1 - 0.1181…
    let v = eval(
        "IF(OR(User.ShapeLayout<>1), Height-User.InfoPanelHeight/2, User.InfoPanelHeight/2)",
        &e,
    )
    .unwrap()
    .as_f64()
    .unwrap();
    assert!((v - (1.0 - 0.2362204724409449 / 2.0)).abs() < 1e-12);
    e.insert("User.ShapeLayout".into(), 1.0);
    let v = eval(
        "IF(OR(User.ShapeLayout<>1), Height-User.InfoPanelHeight/2, User.InfoPanelHeight/2)",
        &e,
    )
    .unwrap()
    .as_f64()
    .unwrap();
    assert!((v - 0.2362204724409449 / 2.0).abs() < 1e-12);
}

// ---- #267 field set ----

fn field_env() -> (Env, StrEnv) {
    (Env::new(), StrEnv::new())
}

fn sample_ctx() -> FieldCtx {
    FieldCtx {
        page_number: Some(2.0),
        page_name: Some("Flowchart-2".to_string()),
        master_name: Some("Decision".to_string()),
        doc_last_save: Some("2012-07-18T23:57:59".to_string()),
        doc_creation: Some("2012-07-26T10:23:29".to_string()),
        doc_creator: Some("Tarahi".to_string()),
        doc_title: Some("Blank Drawing".to_string()),
        doc_subject: None,
        doc_keywords: None,
        doc_description: None,
        units: crate::fieldfmt::UnitSystem::Us,
    }
}

/// The corpus's master-name split, verbatim shape:
/// INDEX(0,INDEX(0,IF(STRSAME(MASTERNAME(),"<no master>"),"",MASTERNAME()),":"),".")
#[test]
fn field_index_mastername_chain() {
    let (env, strs) = field_env();
    let ctx = sample_ctx();
    let v = eval_field(
        "INDEX(0,INDEX(0,IF(STRSAME(MASTERNAME(),\"<no master>\"),\"\",MASTERNAME()),\":\"),\".\")",
        &env,
        &strs,
        Some(&ctx),
    )
    .unwrap();
    assert_eq!(v, Value::Str("Decision".to_string()));

    // No master → empty string through the whole chain.
    let mut ctx2 = sample_ctx();
    ctx2.master_name = None;
    let v = eval_field(
        "INDEX(0,INDEX(0,IF(STRSAME(MASTERNAME(),\"<no master>\"),\"\",MASTERNAME()),\":\"),\".\")",
        &env,
        &strs,
        Some(&ctx2),
    )
    .unwrap();
    assert_eq!(v, Value::Str(String::new()));

    // The omitted delimiter is ";" ([MS-Learn] INDEX function;
    // CodeRabbit round 1).
    assert_eq!(
        eval_field("INDEX(1,\"red;blue\")", &env, &strs, None).unwrap(),
        Value::Str("blue".to_string())
    );
}

#[test]
fn field_strsame_is_case_insensitive() {
    let (env, strs) = field_env();
    assert_eq!(
        eval_field("STRSAME(\"abc\",\"ABC\")", &env, &strs, None).unwrap(),
        Value::Num(1.0)
    );
    assert_eq!(
        eval_field("STRSAME(\"abc\",\"abd\")", &env, &strs, None).unwrap(),
        Value::Num(0.0)
    );
}

#[test]
fn field_string_cells_resolve_in_formulas() {
    let (mut env, mut strs) = field_env();
    strs.insert("User.Label".to_string(), "Hello".to_string());
    env.insert("Width".to_string(), 2.0);
    // A bare string ref, and a concat mixing both envs.
    assert_eq!(
        eval_field("User.Label", &env, &strs, None).unwrap(),
        Value::Str("Hello".to_string())
    );
    assert_eq!(
        eval_field("User.Label&\" w=\"&Width*2", &env, &strs, None).unwrap(),
        Value::Str("Hello w=4".to_string())
    );
    // Numeric-first: a cell present in both resolves as a number.
    strs.insert("Width".to_string(), "ignored".to_string());
    assert_eq!(
        eval_field("Width", &env, &strs, None).unwrap(),
        Value::Num(2.0)
    );
}

#[test]
fn field_page_and_doc_facts() {
    let (env, strs) = field_env();
    let ctx = sample_ctx();
    assert_eq!(
        eval_field("PAGENAME()", &env, &strs, Some(&ctx)).unwrap(),
        Value::Str("Flowchart-2".to_string())
    );
    assert_eq!(
        eval_field("PAGENUMBER()", &env, &strs, Some(&ctx)).unwrap(),
        Value::Num(2.0)
    );
    assert_eq!(
        eval_field("DOCLASTSAVE()", &env, &strs, Some(&ctx)).unwrap(),
        Value::Str("2012-07-18T23:57:59".to_string())
    );
    assert_eq!(
        eval_field("DOCLASTEDIT()", &env, &strs, Some(&ctx)).unwrap(),
        Value::Str("2012-07-18T23:57:59".to_string())
    );
    assert_eq!(
        eval_field("DOCCREATION()", &env, &strs, Some(&ctx)).unwrap(),
        Value::Str("2012-07-26T10:23:29".to_string())
    );
    // Without a context they stay unresolvable (the cached V
    // serves — a master contact sheet renders page-less).
    assert!(eval_field("PAGENUMBER()", &env, &strs, None).is_err());
}

#[test]
fn field_now_is_deliberately_unresolvable() {
    let (env, strs) = field_env();
    assert!(eval_field("NOW()", &env, &strs, Some(&sample_ctx())).is_err());
}

/// DATE composes into the corpus-pinned Excel epoch:
/// DATE(2015,1,1) = 42005.
#[test]
fn field_date_serial_epoch() {
    let (env, strs) = field_env();
    assert_eq!(
        eval_field("DATE(2015,1,1)", &env, &strs, None).unwrap(),
        Value::Num(42005.0)
    );
    assert_eq!(
        eval_field("DATE(2013,1,1)", &env, &strs, None).unwrap(),
        Value::Num(41275.0)
    );
    // Out-of-range arguments ERROR (the cached-V path serves)
    // instead of overflowing the civil-date arithmetic
    // (CodeRabbit round 3).
    assert!(eval_field("DATE(1E30,1,1)", &env, &strs, None).is_err());
    assert!(eval_field("DATE(2015,13,1)", &env, &strs, None).is_err());
    assert!(eval_field("DATE(2015,1,32)", &env, &strs, None).is_err());
}

/// The corpus's date-field composition: FORMAT(User.Date,"d")
/// over a serial, and the FORMATEX weekday probe.
#[test]
fn field_format_and_formatex() {
    let (env, mut strs) = field_env();
    strs.insert("User.Date".to_string(), "42005".to_string());
    assert_eq!(
        eval_field("FORMAT(User.Date,\"{{MMM yyyy}}\")", &env, &strs, None).unwrap(),
        Value::Str("Jan 2015".to_string())
    );
    assert_eq!(
        eval_field("FORMAT(User.Date,\"d\")", &env, &strs, None).unwrap(),
        Value::Str("1".to_string())
    );
    // FORMATEX(1,"{{DD}}",,,1033) → the 2-letter weekday of
    // serial 1 (Sunday): the corpus's locale probe expects "Su".
    assert_eq!(
        eval_field("FORMATEX(1,\"{{DD}}\",,,1033)", &env, &strs, None).unwrap(),
        Value::Str("Su".to_string())
    );
    // Explicit unit conversion converts ONCE (CodeRabbit round 1:
    // 1 in → 2.54 cm, not the double-converted 6.45).
    assert_eq!(
        eval_field("FORMATEX(1,\"0.00u\",\"in\",\"cm\")", &env, &strs, None).unwrap(),
        Value::Str("2.54cm.".to_string())
    );
    // The conversion applies with or without the `u` label — a
    // bare number picture used to skip it and print the raw
    // internal-inch count (Greptile round 2: `FORMATEX(1,"0.00",
    // "ft")` returned `12.00`, not `1.00`).
    assert_eq!(
        eval_field("FORMATEX(1,\"0.00\",\"ft\")", &env, &strs, None).unwrap(),
        Value::Str("1.00".to_string())
    );
    assert_eq!(
        eval_field("FORMATEX(1,\"0.00\",\"in\",\"cm\")", &env, &strs, None).unwrap(),
        Value::Str("2.54".to_string())
    );
}

#[test]
fn field_fieldpicture_table() {
    let (env, strs) = field_env();
    assert_eq!(
        eval_field("FIELDPICTURE(0)", &env, &strs, None).unwrap(),
        Value::Str("esc(0)".to_string())
    );
    assert_eq!(
        eval_field("FIELDPICTURE(37)", &env, &strs, None).unwrap(),
        Value::Str("@".to_string())
    );
    // A live Format cell: FIELDPICTURE(8) formats a width.
    assert_eq!(
        eval_field(
            "FORMAT(2,FIELDPICTURE(8))",
            &env,
            &strs,
            Some(&sample_ctx())
        )
        .unwrap(),
        Value::Str("2.000".to_string())
    );
}
