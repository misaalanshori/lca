use super::{MathMode, render_latex, render_math};

// Verifies: TUI-10 M3 - pi's SYMBOLS table and relation spacing
// (latex.ts dispatch): a glyph command renders its Unicode character.
#[test]
fn symbol_commands_render_their_glyphs() {
    assert_eq!(
        render_latex(r"\alpha + \beta", false).as_deref(),
        Some("α + β")
    );
    assert_eq!(render_latex(r"x \to y", false).as_deref(), Some("x → y"));
    assert_eq!(render_latex(r"a \le b", false).as_deref(), Some("a ≤ b"));
}

// Verifies: M3 - inline fractions slash, display fractions stack
// (formatFraction vs the FractionNode layout).
#[test]
fn fractions_slash_inline_and_stack_in_display() {
    assert_eq!(
        render_latex(r"\frac{a}{b} = \frac{c}{d}", false).as_deref(),
        Some("a/b = c/d")
    );
    assert_eq!(
        render_latex(r"\frac{a}{b}", true).as_deref(),
        Some("a\n─\nb")
    );
    assert_eq!(
        render_latex(r"\frac{x+1}{y-1}", false).as_deref(),
        Some("(x+1)/(y-1)")
    );
}

// Verifies: M3 - roots and their degrees (formatRoot, cube/root-4).
#[test]
fn roots_take_their_degrees() {
    assert_eq!(render_latex(r"\sqrt{x}", false).as_deref(), Some("√x"));
    assert_eq!(
        render_latex(r"\sqrt{x+y}", false).as_deref(),
        Some("√(x+y)")
    );
    assert_eq!(render_latex(r"\sqrt[3]{x}", false).as_deref(), Some("∛x"));
    assert_eq!(render_latex(r"\sqrt[4]{x}", false).as_deref(), Some("∜x"));
}

// Verifies: M3 - scripts: the Unicode maps when they cover the value,
// the ^()/ fallback when they do not (formatScript).
#[test]
fn scripts_use_unicode_or_the_fallback_form() {
    assert_eq!(render_latex("x^2", false).as_deref(), Some("x²"));
    assert_eq!(render_latex("x_i", false).as_deref(), Some("xᵢ"));
    assert_eq!(render_latex("x^{10}", false).as_deref(), Some("x¹⁰"));
    assert_eq!(render_latex("x^{AB}", false).as_deref(), Some("x^(AB)"));
    assert_eq!(render_latex("x^{n+1}", false).as_deref(), Some("xⁿ⁺¹"));
    assert_eq!(render_latex("a_1 b_2", false).as_deref(), Some("a₁ b₂"));
}

// Verifies: M3 - limit operators take their bounds in brackets inline
// (latex.ts parseOperator, "bracket" style).
#[test]
fn limits_take_bracketed_bounds_inline() {
    let out = render_latex(r"\lim_{x \to 0} f(x)", false).unwrap();
    assert!(out.contains("lim[x→0]"), "{out:?}");
    let out = render_latex(r"\max_{i} a_i", false).unwrap();
    assert!(out.contains("max"), "{out:?}");
}

// Verifies: M3 - display-mode big operators stack their limits
// (OperatorNode over DISPLAY_LIMIT_SYMBOLS).
#[test]
fn display_big_operators_stack_their_limits() {
    let out = render_latex(r"\sum_{i=1}^{n} i", true).unwrap();
    assert!(out.contains('∑'), "{out:?}");
    assert!(out.contains("i=1"), "{out:?}");
    assert!(out.contains('n'), "{out:?}");
    assert!(out.lines().count() > 1, "stacked: {out:?}");
    // Inline keeps it flat.
    let out = render_latex(r"\sum_{i=1}^{n} i", false).unwrap();
    assert!(!out.contains('∑') || out.lines().count() == 1, "{out:?}");
}

// Verifies: M3 - matrices: rows split on `\\`, columns joined with `│`,
// and the pmatrix family draws its own delimiters (renderMatrix).
#[test]
fn matrices_draw_columns_and_delimiters() {
    let out = render_latex(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", false).unwrap();
    assert!(out.contains('⎛') && out.contains('⎞'), "{out:?}");
    assert!(out.contains("│"), "{out:?}");
    assert!(out.contains('a') && out.contains('d'), "{out:?}");

    let out = render_latex(r"\begin{vmatrix} 1 & 2 \\ 3 & 4 \end{vmatrix}", false).unwrap();
    assert!(out.contains('│'), "{out:?}");

    let out = render_latex(r"\begin{matrix} a & b \end{matrix}", false).unwrap();
    assert!(
        !out.contains('⎛'),
        "a bare matrix has no delimiters: {out:?}"
    );
}

// Verifies: M3 - cases: the ⎧⎨⎩ brace, value-column alignment with the
// protected space, and the if/when/for/otherwise prefix (renderCases).
#[test]
fn cases_draw_the_brace_and_prefix_conditions() {
    let out = render_latex(
        r"\begin{cases} x & if x > 0 \\ -x & otherwise \end{cases}",
        false,
    )
    .unwrap();
    assert!(out.contains('⎧'), "{out:?}");
    assert!(out.contains('⎩'), "{out:?}");
    assert!(out.contains("if x > 0"), "{out:?}");
    // `otherwise` already reads as the condition: no extra `if`.
    assert!(!out.contains("if otherwise"), "{out:?}");
}

// Verifies: M3 - the aligned family renders one row per `\\` line.
#[test]
fn aligned_environments_render_row_per_line() {
    let out = render_latex(r"\begin{aligned} a &= b \\ c &= d \end{aligned}", false).unwrap();
    assert_eq!(out.lines().count(), 2, "{out:?}");
    assert!(out.contains('a') && out.contains('d'), "{out:?}");
}

// Verifies: M3 - accents compose on a single character and fall back to
// the command form for a run (ACCENTS).
#[test]
fn accents_compose_or_fall_back() {
    assert_eq!(render_latex(r"\hat{x}", false).as_deref(), Some("x\u{302}"));
    assert_eq!(render_latex(r"\hat{xy}", false).as_deref(), Some("hat(xy)"));
    assert_eq!(
        render_latex(r"\vec{v}", false).as_deref(),
        Some("v\u{20d7}")
    );
}

// Verifies: M3 - mathbb maps the blackboard letters, `\not` uses the
// negation table and the U+0338 overlay fallback.
#[test]
fn mathbb_and_negation_follow_the_tables() {
    assert_eq!(render_latex(r"\mathbb{R}", false).as_deref(), Some("ℝ"));
    assert_eq!(render_latex(r"\mathbb{Rx}", false).as_deref(), Some("ℝx"));
    assert_eq!(render_latex(r"\not=", false).as_deref(), Some("≠"));
    assert_eq!(render_latex(r"a \not= b", false).as_deref(), Some("a ≠ b"));
    assert_eq!(render_latex(r"x \notin y", false).as_deref(), Some("x ∉ y"));
}

// Verifies: M3 - the named-operator sentinels put spaces where pi's
// normalizeOutput patterns do (`x\sin y` needs one before `sin`).
#[test]
fn named_operators_get_pis_spacing() {
    assert_eq!(render_latex(r"\sin x", false).as_deref(), Some("sin x"));
    assert_eq!(render_latex(r"x\sin y", false).as_deref(), Some("x sin y"));
    assert_eq!(
        render_latex(r"\cos^2\theta", false).as_deref(),
        Some("cos² θ")
    );
}

// Verifies: M3 - the wrappers and one-liners pi handles: boxed, binom,
// mod forms, overset/underset, and the font switches that vanish.
#[test]
fn the_misc_constructs_pi_handles() {
    assert_eq!(
        render_latex(r"\boxed{a+b}", false).as_deref(),
        Some("[a+b]")
    );
    assert_eq!(
        render_latex(r"\binom{n}{k}", false).as_deref(),
        Some("(n choose k)")
    );
    assert_eq!(
        render_latex(r"a \bmod b", false).as_deref(),
        Some("a mod b")
    );
    assert_eq!(
        render_latex(r"a \pmod{3}", false).as_deref(),
        Some("a (mod 3)")
    );
    assert_eq!(
        render_latex(r"\overset{!}{=}", false).as_deref(),
        Some("=^!")
    );
    assert_eq!(render_latex(r"\bf x", false).as_deref(), Some("x"));
}

// Verifies: M3's fail-soft contract - unsupported or malformed input
// returns None so markdown shows the raw source; never half a render.
#[test]
fn unsupported_or_malformed_input_fails_soft() {
    assert_eq!(render_latex(r"\unknowncmd", false), None);
    assert_eq!(render_latex(r"\frac{a", false), None);
    assert_eq!(render_latex("{a", false), None);
    assert_eq!(render_latex(r"a}", false), None);
    assert_eq!(render_latex(r"\begin{nope} x \end{nope}", false), None);
}

// Verifies: M3 - the public modes map to pi's display flag.
#[test]
fn the_two_modes_map_to_pi_display_flag() {
    assert_eq!(
        render_math(r"\frac{a}{b}", MathMode::Inline).as_deref(),
        Some("a/b")
    );
    assert_eq!(
        render_math(r"\frac{a}{b}", MathMode::Display).as_deref(),
        Some("a\n─\nb")
    );
}
