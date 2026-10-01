//! A terminal LaTeX typesetter: pi's `packages/tui/src/latex.ts`, ported
//! behavior-for-behavior (RE `tui-widgets/latex.md`).
//!
//! `render_latex(source, display)` returns the expression as Unicode
//! text (multi-line for stacked forms), or `None` for unsupported or
//! malformed input, which the markdown renderer shows as the raw source
//! (the deliberate fail-soft contract: never half-render).
//!
//! The port keeps pi's two-pass shape: the parser writes a flat string
//! that may embed layout markers (`MARKER_START` + node index), and the
//! layout module composes those nodes into baseline-joined blocks at the
//! end. The symbol tables are data only.

mod layout;
mod symbols;

use layout::{
    MARKER_END, MARKER_START, NAMED_END, NEGATIVE_SPACE, Node, PROTECTED_SPACE, format_fraction,
    format_root, format_script, format_unicode_script, normalize_output, render_layout,
};

/// Named-operator open sentinel (pi's `NAMED_OPERATOR_START`).
const NAMED_START: char = '\u{f0004}';

/// Render a LaTeX math expression as terminal-friendly Unicode text.
///
/// Returns `None` for unsupported or malformed syntax - the caller shows
/// the raw source instead (pi's `renderLatex` returning `undefined`).
pub fn render_latex(source: &str, display: bool) -> Option<String> {
    let mut nodes = Vec::new();
    let mut parser = Parser::new(source, display);
    let rendered = parser.render(&mut nodes)?;
    if nodes.is_empty() {
        return Some(rendered.replace(PROTECTED_SPACE, " "));
    }
    let lines = render_layout(&rendered, &nodes).lines;
    let indentation = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    let out = lines
        .iter()
        .map(|line| {
            line.chars()
                .skip(indentation)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<String>>()
        .join("\n")
        .trim_end()
        .to_string();
    Some(out.replace(PROTECTED_SPACE, " "))
}

/// Math rendering modes: markdown passes pi's `renderLatex` display flag
/// straight through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathMode {
    /// `$x$` and friends: scripts stay inline-sized.
    Inline,
    /// `$$x$$` and friends: fractions and limits stack.
    Display,
}

impl MathMode {
    /// Whether this is display math (pi's `{ display: true }`).
    fn is_display(self) -> bool {
        matches!(self, MathMode::Display)
    }
}

/// Render with an explicit mode, the shape the markdown layer calls.
pub fn render_math(source: &str, mode: MathMode) -> Option<String> {
    render_latex(source, mode.is_display())
}

fn is_ws(c: char) -> bool {
    c.is_whitespace()
}

/// pi's `[A-Za-z]` test (LaTeX command names are ASCII letters).
fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// `if/when/for/otherwise` at the head of a cases condition (word-bounded).
fn starts_with_word(condition: &str) -> bool {
    ["if", "when", "for", "otherwise"].iter().any(|word| {
        condition
            .strip_prefix(word)
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// The node index when `result` ends in a layout marker (pi's
/// `TRAILING_LAYOUT_MARKER_PATTERN`), so a source `.` can attach to it.
fn trailing_matrix_index(result: &str) -> Option<usize> {
    let chars: Vec<char> = result.chars().collect();
    let end = chars.len();
    if end == 0 || chars[end - 1] != MARKER_END {
        return None;
    }
    let mut start = end - 1;
    while start > 0 && chars[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == 0 || chars[start - 1] != MARKER_START {
        return None;
    }
    chars[start..end - 1]
        .iter()
        .collect::<String>()
        .parse()
        .ok()
}

/// Find `needle` (chars) in `haystack`, returning its char index.
fn find_subslice(haystack: &str, needle: &[char]) -> Option<usize> {
    let hay: Vec<char> = haystack.chars().collect();
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

struct Parser {
    source: Vec<char>,
    display: bool,
    position: usize,
    supported: bool,
    stack_fractions: bool,
    script_depth: usize,
}

impl Parser {
    fn new(source: &str, display: bool) -> Self {
        Self {
            source: source.chars().collect(),
            display,
            position: 0,
            supported: true,
            stack_fractions: true,
            script_depth: 0,
        }
    }

    fn render(&mut self, nodes: &mut Vec<Node>) -> Option<String> {
        let result = self.parse_sequence(None, nodes);
        if !self.supported || self.position != self.source.len() {
            return None;
        }
        Some(normalize_output(&result))
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.position).copied()
    }

    /// pi's `parseSequence`: groups, commands, scripts, whitespace and the
    /// punctuation rules, ending early at `end` when one is expected.
    fn parse_sequence(&mut self, end: Option<char>, nodes: &mut Vec<Node>) -> String {
        let mut result = String::new();
        while self.position < self.source.len() {
            let character = self.source[self.position];
            if let Some(end) = end
                && character == end
            {
                self.position += 1;
                return result;
            }
            if character == '}' {
                self.supported = false;
                return result;
            }
            if character == '{' {
                self.position += 1;
                result.push_str(&self.parse_sequence(Some('}'), nodes));
                continue;
            }
            if character == '\\' {
                let command = self.parse_command(nodes);
                if command == NEGATIVE_SPACE.to_string() {
                    while result.ends_with(' ') {
                        result.pop();
                    }
                    if result.ends_with(NAMED_END) {
                        result.pop();
                    }
                } else {
                    result.push_str(&command);
                }
                continue;
            }
            if character == '^' || character == '_' {
                self.position += 1;
                while result.ends_with(' ') {
                    result.pop();
                }
                let script = self.parse_scripts(character, nodes);
                if result.ends_with(NAMED_END) {
                    result.pop();
                    result.push_str(&script);
                    result.push(NAMED_END);
                } else {
                    result.push_str(&script);
                }
                continue;
            }
            if is_ws(character) {
                result.push_str(&self.parse_whitespace());
                continue;
            }
            if matches!(character, '=' | '<' | '>') {
                while result.ends_with(' ') {
                    result.pop();
                }
                result.push(' ');
                result.push(character);
                result.push(' ');
                self.position += 1;
                continue;
            }
            if character == '&' {
                self.position += 1;
                continue;
            }
            if character == '~' {
                self.position += 1;
                result.push(' ');
                continue;
            }
            if character == '.'
                && let Some(index) = trailing_matrix_index(&result)
                && let Some(Node::Matrix { lines, .. }) = nodes.get_mut(index)
            {
                // pi: a `.` right after a matrix marker appends to the
                // matrix's last line instead of starting new text.
                if let Some(last) = lines.last_mut() {
                    last.push('.');
                }
                self.position += 1;
                continue;
            }
            result.push(character);
            self.position += 1;
        }
        if end.is_some() {
            self.supported = false;
        }
        result
    }

    fn parse_whitespace(&mut self) -> String {
        while self.source.get(self.position).is_some_and(|c| is_ws(*c)) {
            self.position += 1;
        }
        " ".to_string()
    }

    /// pi's `parseScripts`: sub/sup in either order, Unicode-mapped when
    /// possible, layout-stacked in display mode otherwise.
    fn parse_scripts(&mut self, initial: char, nodes: &mut Vec<Node>) -> String {
        let mut sub: Option<String> = None;
        let mut sup: Option<String> = None;
        let mut order: Vec<char> = Vec::new();
        self.script_depth += 1;
        let value = self.parse_required_argument(false, nodes);
        self.script_depth -= 1;
        if initial == '_' {
            sub = Some(value);
        } else {
            sup = Some(value);
        }
        order.push(if initial == '_' { 's' } else { 'u' });

        let mut next = self.position;
        while self.source.get(next).is_some_and(|c| is_ws(*c)) {
            next += 1;
        }
        if let Some(marker) = self.source.get(next).copied()
            && (marker == '^' || marker == '_')
            && marker != initial
        {
            self.position = next + 1;
            self.script_depth += 1;
            let value = self.parse_required_argument(false, nodes);
            self.script_depth -= 1;
            if marker == '_' {
                sub = Some(value);
            } else {
                sup = Some(value);
            }
            order.push(if marker == '_' { 's' } else { 'u' });
        }

        let sub_unicode = sub.as_deref().and_then(|v| format_unicode_script(v, true));
        let sup_unicode = sup.as_deref().and_then(|v| format_unicode_script(v, false));
        let can_use_layout = ![sub.as_deref(), sup.as_deref()].iter().any(|value| {
            value.is_some_and(|value| {
                value.contains('/')
                    || (!value.contains(MARKER_START)
                        && value.chars().count() > 1
                        && !value
                            .chars()
                            .any(|c| c.is_ascii_uppercase() || c == '*' || c == '∗'))
            })
        });
        let needs_layout = self.display
            && can_use_layout
            && (self.script_depth > 0
                || (sub.is_some() && sub_unicode.is_none())
                || (sup.is_some() && sup_unicode.is_none()));
        if !needs_layout {
            return order
                .iter()
                .map(|kind| {
                    if *kind == 's' {
                        let value = sub.clone().unwrap_or_default();
                        sub_unicode
                            .clone()
                            .unwrap_or_else(|| format_script(&value, true))
                    } else {
                        let value = sup.clone().unwrap_or_default();
                        sup_unicode
                            .clone()
                            .unwrap_or_else(|| format_script(&value, false))
                    }
                })
                .collect();
        }
        let index = nodes.len();
        nodes.push(Node::Script {
            lower: sub.map(|v| normalize_output(&v)),
            upper: sup.map(|v| normalize_output(&v)),
        });
        format!("{MARKER_START}{index}{MARKER_END}")
    }
}

impl Parser {
    /// pi's `parseCommand`: the dispatch table behind every `\command`.
    fn parse_command(&mut self, nodes: &mut Vec<Node>) -> String {
        self.position += 1;
        if self.position >= self.source.len() {
            self.supported = false;
            return String::new();
        }
        let first = self.source[self.position];
        if first == '\n' || first == '\r' {
            self.position += 1;
            if first == '\r' && self.peek() == Some('\n') {
                self.position += 1;
            }
            return " ".to_string();
        }
        let command = if is_letter(first) {
            let start = self.position;
            while self
                .source
                .get(self.position)
                .is_some_and(|c| is_letter(*c))
            {
                self.position += 1;
            }
            self.source[start..self.position].iter().collect::<String>()
        } else {
            self.position += 1;
            first.to_string()
        };

        if command == "\\" {
            return "\n".to_string();
        }
        if symbols::SPACING_COMMANDS.contains(&command.as_str()) {
            return " ".to_string();
        }
        if symbols::NEGATIVE_SPACING_COMMANDS.contains(&command.as_str()) {
            return NEGATIVE_SPACE.to_string();
        }
        if symbols::FONT_SWITCH_COMMANDS.contains(&command.as_str()) {
            while self.source.get(self.position).is_some_and(|c| is_ws(*c)) {
                self.position += 1;
            }
            return String::new();
        }
        if symbols::IGNORED_COMMANDS.contains(&command.as_str()) {
            return String::new();
        }
        if matches!(command.as_str(), "{" | "}" | "$" | "%" | "#" | "_" | "&") {
            return command;
        }
        if command == "|" {
            return "‖".to_string();
        }
        if command == "not" {
            let value = self
                .parse_required_argument(false, nodes)
                .trim()
                .to_string();
            if let Some((_, negated)) = symbols::NEGATED_SYMBOLS.iter().find(|(k, _)| *k == value) {
                return format!(" {negated} ");
            }
            let mut chars = value.chars();
            let Some(first_char) = chars.next() else {
                self.supported = false;
                return String::new();
            };
            let rest: String = chars.collect();
            return format!(" {first_char}\u{338}{rest} ");
        }
        if symbols::LIMIT_OPERATORS.contains(&command.as_str()) {
            return self.parse_operator(&command, true, true, true, nodes);
        }
        if let Some((_, symbol)) = symbols::SYMBOLS.iter().find(|(k, _)| *k == command) {
            let symbol = (*symbol).to_string();
            if symbols::DISPLAY_LIMIT_SYMBOLS.contains(&command.as_str()) {
                return self.parse_operator(&symbol, false, true, false, nodes);
            }
            if command == "cdot"
                || command == "times"
                || symbols::RELATION_COMMANDS.contains(&command.as_str())
            {
                return format!(" {symbol} ");
            }
            return symbol;
        }
        if symbols::NAMED_OPERATORS.contains(&command.as_str()) {
            return format!("{NAMED_START}{command}{NAMED_END}");
        }
        if symbols::SIZE_COMMANDS.contains(&command.as_str()) {
            return String::new();
        }
        if matches!(command.as_str(), "left" | "middle" | "right") {
            if self.peek() == Some('.') {
                self.position += 1;
            }
            return String::new();
        }
        if matches!(command.as_str(), "frac" | "dfrac" | "tfrac") {
            let should_stack = self.display && self.stack_fractions && command != "tfrac";
            let numerator = self.parse_required_argument(!should_stack, nodes);
            let denominator = self.parse_required_argument(!should_stack, nodes);
            if should_stack {
                let index = nodes.len();
                nodes.push(Node::Fraction {
                    numerator: normalize_output(&numerator),
                    denominator: normalize_output(&denominator),
                });
                return format!("{MARKER_START}{index}{MARKER_END}");
            }
            return format_fraction(&numerator, &denominator);
        }
        if command == "sqrt" {
            let degree = self
                .parse_optional_argument(nodes)
                .map(|value| value.trim().to_string());
            let value = self.parse_required_argument(true, nodes);
            return match degree.as_deref() {
                None | Some("2") => format_root(&value, "√"),
                Some("3") => format_root(&value, "∛"),
                Some("4") => format_root(&value, "∜"),
                Some(degree) => {
                    format!(
                        "{}{}",
                        format_script(degree, false),
                        format_root(&value, "√")
                    )
                }
            };
        }
        if matches!(command.as_str(), "boxed" | "fbox") {
            return format!("[{}]", self.parse_required_argument(true, nodes).trim());
        }
        if matches!(command.as_str(), "binom" | "dbinom" | "tbinom") {
            let top = self.parse_required_argument(true, nodes);
            let bottom = self.parse_required_argument(true, nodes);
            return format!("({top} choose {bottom})");
        }
        if let Some((_, accent)) = symbols::ACCENTS.iter().find(|(k, _)| *k == command) {
            let value = self.parse_required_argument(true, nodes);
            let accent = (*accent).to_string();
            return if value.chars().count() == 1 {
                format!("{value}{accent}")
            } else {
                format!("{command}({value})")
            };
        }
        if command == "mathbb" {
            let value = self.parse_required_argument(true, nodes);
            return value
                .chars()
                .map(|c| {
                    let key = c.to_string();
                    symbols::BLACKBOARD
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map_or(key.clone(), |(_, v)| (*v).to_string())
                })
                .collect();
        }
        if command == "operatorname" {
            let starred = self.peek() == Some('*');
            if starred {
                self.position += 1;
            }
            let operator = normalize_output(&self.parse_required_argument(true, nodes))
                .trim()
                .to_string();
            return self.parse_operator(&operator, true, starred, true, nodes);
        }
        if command == "mod" || command == "bmod" {
            return " mod ".to_string();
        }
        if command == "pmod" || command == "pod" {
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return if command == "pmod" {
                format!(" (mod {value})")
            } else {
                format!(" ({value})")
            };
        }
        if matches!(command.as_str(), "overset" | "stackrel") {
            let upper = self.parse_required_argument(true, nodes);
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return format!("{value}{}", format_script(&upper, false));
        }
        if command == "underset" {
            let lower = self.parse_required_argument(true, nodes);
            let value = self.parse_required_argument(true, nodes).trim().to_string();
            return format!("{value}{}", format_script(&lower, true));
        }
        if symbols::PLAIN_WRAPPERS.contains(&command.as_str()) {
            let value = self.parse_required_argument(true, nodes);
            return if command.starts_with("text") || command == "mbox" {
                value
            } else {
                value.trim().to_string()
            };
        }
        if command == "begin" {
            return self.parse_environment(nodes);
        }
        if command == "end" {
            self.supported = false;
            return String::new();
        }
        self.supported = false;
        format!("\\{command}")
    }
}

impl Parser {
    /// pi's `parseOperator`: `\limits` lookahead, bound collection, and
    /// the display-mode stacked form.
    fn parse_operator(
        &mut self,
        operator: &str,
        inline_lower_bracket: bool,
        display_limits: bool,
        spaced: bool,
        nodes: &mut Vec<Node>,
    ) -> String {
        let mut use_display_limits = display_limits;
        // `\\(limits|nolimits)(?![A-Za-z])` after optional horizontal space.
        let mut modifier = self.position;
        while self
            .source
            .get(modifier)
            .is_some_and(|c| *c == ' ' || *c == '\t')
        {
            modifier += 1;
        }
        if self.source.get(modifier) == Some(&'\\') {
            let rest: String = self.source[modifier + 1..].iter().collect();
            for name in ["limits", "nolimits"] {
                if let Some(after) = rest.strip_prefix(name)
                    && after.chars().next().is_none_or(|c| !is_letter(c))
                {
                    use_display_limits = name == "limits";
                    self.position = modifier + 1 + name.len();
                    break;
                }
            }
        }

        let mut lower: Option<String> = None;
        let mut upper: Option<String> = None;
        loop {
            let mut script_at = self.position;
            while self
                .source
                .get(script_at)
                .is_some_and(|c| *c == ' ' || *c == '\t')
            {
                script_at += 1;
            }
            let Some(&kind) = self.source.get(script_at) else {
                break;
            };
            if kind != '_' && kind != '^' {
                break;
            }
            self.position = script_at + 1;
            let value = normalize_output(&self.parse_required_argument(false, nodes))
                .chars()
                .filter(|c| *c != ' ')
                .collect::<String>();
            if kind == '_' {
                if lower.is_some() {
                    self.supported = false;
                }
                lower = Some(value);
            } else {
                if upper.is_some() {
                    self.supported = false;
                }
                upper = Some(value);
            }
        }

        if self.display && use_display_limits && (lower.is_some() || upper.is_some()) {
            let index = nodes.len();
            nodes.push(Node::Operator {
                operator: operator.to_string(),
                lower,
                upper,
            });
            return format!("{MARKER_START}{index}{MARKER_END}");
        }

        let mut rendered = operator.to_string();
        if let Some(lower) = lower {
            if inline_lower_bracket {
                rendered.push('[');
                rendered.push_str(&lower);
                rendered.push(']');
            } else {
                rendered.push_str(&format_script(&lower, true));
            }
        }
        if let Some(upper) = upper {
            rendered.push_str(&format_script(&upper, false));
        }
        if spaced {
            format!(" {rendered} ")
        } else {
            rendered
        }
    }

    fn parse_required_argument(&mut self, stack: bool, nodes: &mut Vec<Node>) -> String {
        let previous = self.stack_fractions;
        self.stack_fractions = previous && stack;
        let value = self.parse_required_argument_value(nodes);
        self.stack_fractions = previous;
        value
    }

    fn parse_required_argument_value(&mut self, nodes: &mut Vec<Node>) -> String {
        while self.source.get(self.position).is_some_and(|c| is_ws(*c)) {
            self.position += 1;
        }
        if self.position >= self.source.len() {
            self.supported = false;
            return String::new();
        }
        if self.source[self.position] == '{' {
            self.position += 1;
            return self.parse_sequence(Some('}'), nodes);
        }
        if self.source[self.position] == '\\' {
            return self.parse_command(nodes);
        }
        let value = self.source[self.position];
        self.position += 1;
        value.to_string()
    }

    fn parse_optional_argument(&mut self, nodes: &mut Vec<Node>) -> Option<String> {
        while self
            .source
            .get(self.position)
            .is_some_and(|c| *c == ' ' || *c == '\t')
        {
            self.position += 1;
        }
        if self.peek() != Some('[') {
            return None;
        }
        let rest: String = self.source[self.position + 1..].iter().collect();
        let end = rest.find(']')?;
        let value: String = rest[..end].chars().collect();
        self.position += end + 2;
        Some(self.render_nested(&value, true, nodes))
    }

    /// A `{...}` read literally, backslash-aware (pi's `readRawGroup`).
    fn read_raw_group(&mut self) -> Option<String> {
        while self
            .source
            .get(self.position)
            .is_some_and(|c| *c == ' ' || *c == '\t')
        {
            self.position += 1;
        }
        if self.peek() != Some('{') {
            self.supported = false;
            return None;
        }
        self.position += 1;
        let start = self.position;
        let mut depth = 1usize;
        while self.position < self.source.len() {
            let character = self.source[self.position];
            if character == '\\' {
                self.position += 2;
                continue;
            }
            if character == '{' {
                depth += 1;
            }
            if character == '}' {
                depth -= 1;
                if depth == 0 {
                    let value: String = self.source[start..self.position].iter().collect();
                    self.position += 1;
                    return Some(value);
                }
            }
            self.position += 1;
        }
        self.supported = false;
        None
    }

    /// Split environment rows on `\\` with an optional `[...]` row height
    /// (pi's `splitEnvironmentRows`).
    fn split_environment_rows(body: &str) -> Vec<String> {
        let chars: Vec<char> = body.chars().collect();
        let mut rows = Vec::new();
        let mut start = 0usize;
        let mut i = 0usize;
        while i + 1 < chars.len() {
            if chars[i] == '\\' && chars[i + 1] == '\\' {
                let end = i;
                i += 2;
                if chars.get(i) == Some(&'[') {
                    let mut j = i + 1;
                    while j < chars.len() && chars[j] != ']' && chars[j] != '\n' {
                        j += 1;
                    }
                    if chars.get(j) == Some(&']') {
                        i = j + 1;
                    }
                }
                rows.push(chars[start..end].iter().collect());
                start = i;
                continue;
            }
            i += 1;
        }
        rows.push(chars[start..].iter().collect());
        rows
    }

    /// `^\s*\{[^}]*\}` removal (alignedat's column spec, array's colspec).
    fn strip_leading_brace_group(body: &str) -> &str {
        let trimmed = body.trim_start();
        match trimmed.strip_prefix('{') {
            Some(rest) => match rest.find('}') {
                Some(end) => &rest[end + 1..],
                None => trimmed,
            },
            None => trimmed,
        }
    }

    fn parse_environment(&mut self, nodes: &mut Vec<Node>) -> String {
        let Some(environment) = self.read_raw_group() else {
            return String::new();
        };
        let end_marker: Vec<char> = format!("\\end{{{environment}}}").chars().collect();
        let rest: String = self.source[self.position..].iter().collect();
        let Some(end) = find_subslice(&rest, &end_marker) else {
            self.supported = false;
            return String::new();
        };
        let body: String = rest[..end].chars().collect();
        let consumed: usize = rest[..end].chars().count();
        self.position += consumed + end_marker.len();

        match environment.as_str() {
            "equation" | "equation*" | "displaymath" => {
                self.render_nested(&body, true, nodes).trim().to_string()
            }
            "aligned" | "align" | "align*" | "alignedat" | "alignat" | "alignat*" | "gather"
            | "gathered" | "multline" | "multline*" | "split" => {
                let aligned_at =
                    matches!(environment.as_str(), "alignedat" | "alignat" | "alignat*");
                let aligned_body = if aligned_at {
                    Self::strip_leading_brace_group(&body).to_string()
                } else {
                    body
                };
                Self::split_environment_rows(&aligned_body)
                    .into_iter()
                    .filter_map(|row| {
                        let cells: Vec<&str> = row.split('&').collect();
                        let source = if aligned_at {
                            cells
                                .chunks(2)
                                .map(|pair| pair.join(""))
                                .collect::<Vec<_>>()
                                .join(" ")
                        } else {
                            cells.join("")
                        };
                        let rendered = self.render_nested(&source, true, nodes).trim().to_string();
                        (!rendered.is_empty()).then_some(rendered)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "cases" | "cases*" => self.render_cases(&body, nodes),
            "array" | "matrix" | "smallmatrix" | "pmatrix" | "bmatrix" | "Bmatrix" | "vmatrix"
            | "Vmatrix" => {
                let matrix_body = if environment == "array" {
                    Self::strip_leading_brace_group(&body).to_string()
                } else {
                    body
                };
                self.render_matrix(&environment, &matrix_body, nodes)
            }
            _ => {
                self.supported = false;
                body
            }
        }
    }

    fn render_cases(&mut self, body: &str, nodes: &mut Vec<Node>) -> String {
        let rows: Vec<Vec<String>> = Self::split_environment_rows(body)
            .into_iter()
            .map(|row| {
                row.split('&')
                    .map(|cell| self.render_nested(cell, false, nodes).trim().to_string())
                    .collect()
            })
            .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
            .collect();
        let value_width = rows
            .iter()
            .map(|row| {
                let value = row.first().map(String::as_str).unwrap_or("");
                crate::engine::text::visible_width(value.trim_end_matches(", "))
            })
            .max()
            .unwrap_or(0);
        let contents: Vec<String> = rows
            .iter()
            .map(|row| {
                let value = row.first().map(String::as_str).unwrap_or("");
                let value = value.trim_end_matches(", ");
                let condition = row.get(1).map(String::as_str).unwrap_or("");
                if condition.is_empty() {
                    return value.to_string();
                }
                let prefix = if starts_with_word(condition) {
                    " "
                } else {
                    " if "
                };
                let pad = value_width.saturating_sub(crate::engine::text::visible_width(value));
                let padding: String = std::iter::repeat_n(PROTECTED_SPACE, pad).collect();
                format!("{value}{padding}{prefix}{condition}")
            })
            .collect();
        if contents.len() <= 1 {
            return match contents.first() {
                None => String::new(),
                Some(first) => format!("⎧ {first}"),
            };
        }
        let middle = contents.len() / 2;
        let mut visual: Vec<Option<String>> = contents.into_iter().map(Some).collect();
        if visual.len().is_multiple_of(2) {
            visual.insert(middle, None);
        }
        let lines: Vec<String> = visual
            .iter()
            .enumerate()
            .map(|(index, content)| {
                let delimiter = if index == 0 {
                    "⎧"
                } else if index + 1 == visual.len() {
                    "⎩"
                } else {
                    "⎨"
                };
                match content {
                    None => delimiter.to_string(),
                    Some(content) => format!("{delimiter} {content}"),
                }
            })
            .collect();
        let index = nodes.len();
        nodes.push(Node::Matrix {
            lines,
            baseline: middle,
        });
        format!("{MARKER_START}{index}{MARKER_END}")
    }

    fn render_matrix(&mut self, environment: &str, body: &str, nodes: &mut Vec<Node>) -> String {
        let matrix: Vec<Vec<String>> = Self::split_environment_rows(body)
            .into_iter()
            .map(|row| {
                row.split('&')
                    .map(|cell| self.render_nested(cell, false, nodes).trim().to_string())
                    .collect()
            })
            .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
            .collect();
        let columns = matrix.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                matrix
                    .iter()
                    .map(|row| {
                        crate::engine::text::visible_width(
                            row.get(column).map(String::as_str).unwrap_or(""),
                        )
                    })
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rows: Vec<String> = matrix
            .iter()
            .map(|row| {
                (0..columns)
                    .map(|column| {
                        let cell = row.get(column).map(String::as_str).unwrap_or("");
                        let pad =
                            widths[column].saturating_sub(crate::engine::text::visible_width(cell));
                        let padding: String = std::iter::repeat_n(PROTECTED_SPACE, pad).collect();
                        format!("{cell}{padding}")
                    })
                    .collect::<Vec<_>>()
                    .join(" │ ")
            })
            .collect();

        let lines: Vec<String> = if matches!(environment, "array" | "matrix" | "smallmatrix") {
            rows
        } else {
            let Some(delimiter) = symbols::MATRIX_DELIMITERS
                .iter()
                .find(|(name, _)| *name == environment)
                .map(|(_, d)| d)
            else {
                self.supported = false;
                return rows.join("\n");
            };
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let (left, right) = if index == 0 {
                        (delimiter[0], delimiter[1])
                    } else if index + 1 == rows.len() {
                        (delimiter[4], delimiter[5])
                    } else {
                        (delimiter[2], delimiter[3])
                    };
                    format!("{left} {row} {right}")
                })
                .collect()
        };

        if lines.len() <= 1 {
            return lines.first().cloned().unwrap_or_default();
        }
        let index = nodes.len();
        nodes.push(Node::Matrix { lines, baseline: 0 });
        format!("{MARKER_START}{index}{MARKER_END}")
    }

    /// A sub-parser over `source` sharing the node table (pi's
    /// `renderNested`): a nested failure fails the whole expression.
    fn render_nested(&mut self, source: &str, stack: bool, nodes: &mut Vec<Node>) -> String {
        let mut nested = Parser::new(source, self.display && stack);
        match nested.render(nodes) {
            Some(rendered) => rendered,
            None => {
                self.supported = false;
                source.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests;
