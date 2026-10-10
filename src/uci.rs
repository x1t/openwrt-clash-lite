//! Reads and writes the subset of the UCI file format we use: `config <type> '<name>'` sections
//! with `option <key> '<value>'` lines. Quoting follows uci itself ('...', "...", backslash);
//! `package` and `list` lines are accepted and ignored because our config has none.
use crate::state::Res;

#[derive(Debug, PartialEq, Default)]
pub struct Section {
    pub kind: String,
    pub name: Option<String>,
    pub options: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Text up to the closing `quote`; inside double quotes a backslash escapes the next character.
fn quoted(chars: &mut std::str::Chars, quote: char, out: &mut String) -> Res<()> {
    loop {
        match chars.next() {
            Some(c) if c == quote => return Ok(()),
            Some('\\') if quote == '"' => out.push(chars.next().ok_or("dangling backslash")?),
            Some(c) => out.push(c),
            None => return Err(format!("unterminated {quote}").into()),
        }
    }
}

/// Splits a line into words like uci does: adjacent quoted parts join (`'it'\''s'` is `it's`).
fn words(line: &str) -> Res<Vec<String>> {
    let (mut out, mut cur, mut in_word) = (Vec::new(), String::new(), false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => quoted(&mut chars, c, &mut cur)?,
            '\\' => cur.push(chars.next().ok_or("dangling backslash")?),
            '#' if !in_word => break,
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                }
                in_word = false;
                continue;
            }
            c => cur.push(c),
        }
        in_word = true;
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

pub fn parse(text: &str) -> Res<Vec<Section>> {
    let mut sections: Vec<Section> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let w = words(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        match w.first().map(String::as_str) {
            None | Some("package" | "list") => {}
            Some("config") if w.len() >= 2 => sections.push(Section {
                kind: w[1].clone(),
                name: w.get(2).cloned(),
                options: Vec::new(),
            }),
            Some("option") if w.len() >= 2 => sections
                .last_mut()
                .ok_or(format!("line {}: option outside a section", n + 1))?
                .options
                .push((w[1].clone(), w.get(2).cloned().unwrap_or_default())),
            _ => return Err(format!("line {}: cannot parse {line:?}", n + 1).into()),
        }
    }
    Ok(sections)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub fn serialize(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections {
        out.push_str(&format!("config {}", s.kind));
        if let Some(name) = &s.name {
            out.push_str(&format!(" {}", quote(name)));
        }
        out.push('\n');
        for (k, v) in &s.options {
            out.push_str(&format!("\toption {k} {}\n", quote(v)));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_quoting_styles_uci_writes_and_accepts() {
        let text = "\nconfig main 'main'\n\toption a 'single'\n\toption b \"dou\\\"ble\"\n\
                    \toption c bare\n\toption d 'it'\\''s'\n\toption e ''\n  # comment\n";
        let s = &parse(text).unwrap()[0];
        assert_eq!((s.kind.as_str(), s.name.as_deref()), ("main", Some("main")));
        assert_eq!(s.get("a"), Some("single"));
        assert_eq!(s.get("b"), Some("dou\"ble"));
        assert_eq!(s.get("c"), Some("bare"));
        assert_eq!(s.get("d"), Some("it's"));
        assert_eq!(s.get("e"), Some(""));
    }

    #[test]
    fn values_with_spaces_hashes_and_quotes_survive_a_round_trip() {
        let sections = vec![Section {
            kind: "subscription".into(),
            name: Some("R1".into()),
            options: vec![
                ("url".into(), "https://x/a?b=1&c=#frag".into()),
                ("name".into(), "it's \"mine\" now".into()),
            ],
        }];
        assert_eq!(parse(&serialize(&sections)).unwrap(), sections);
    }

    #[test]
    fn unnamed_sections_lists_and_package_lines_are_handled() {
        let text = "package x\nconfig foo\n\tlist l 'a'\n\toption k v\n";
        let s = parse(text).unwrap();
        assert_eq!(s[0].name, None);
        assert_eq!(s[0].options, vec![("k".to_string(), "v".to_string())]);
    }

    #[test]
    fn broken_lines_are_errors_with_the_line_number() {
        assert!(
            parse("config main 'main\n")
                .unwrap_err()
                .to_string()
                .contains("line 1")
        );
        assert!(
            parse("option k v\n")
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        assert!(
            parse("config main\nbogus line\n")
                .unwrap_err()
                .to_string()
                .contains("line 2")
        );
    }

    #[test]
    fn the_last_duplicate_option_wins_like_in_uci() {
        let s = &parse("config main\n\toption k 1\n\toption k 2\n").unwrap()[0];
        assert_eq!(s.get("k"), Some("2"));
    }
}
