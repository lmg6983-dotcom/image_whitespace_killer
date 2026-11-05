//! Output file naming and natural ordering of input files.

use std::cmp::Ordering;

/// Validate a name template. Placeholders:
/// `{num}` sequence number, `{num:03}` zero-padded number, `{stem}` original
/// file name without extension. `{n}` / `{n:03}` are accepted as aliases.
/// `{{` / `}}` produce literal braces.
pub fn validate_template(t: &str) -> Result<(), String> {
    if t.trim().is_empty() {
        return Err("name template must not be empty".into());
    }
    render(t, 1, "x").map(|_| ())
}

pub fn render(t: &str, n: u64, stem: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut key = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => key.push(ch),
                        None => return Err(format!("unclosed '{{' in name template '{t}'")),
                    }
                }
                let spec = key.strip_prefix("num:").or_else(|| key.strip_prefix("n:"));
                match (key.as_str(), spec) {
                    ("num" | "n", _) => out.push_str(&n.to_string()),
                    ("stem", _) => out.push_str(stem),
                    (k, Some(spec)) => {
                        let width: usize = spec.trim_start_matches('0').parse().map_err(|_| {
                            format!("bad number format '{{{k}}}' (use e.g. {{num:03}})")
                        })?;
                        out.push_str(&format!("{n:0width$}"));
                    }
                    (other, None) => {
                        return Err(format!(
                            "unknown placeholder '{{{other}}}' (allowed: {{num}}, {{num:03}}, {{stem}})"
                        ));
                    }
                }
            }
            '}' => return Err(format!("unmatched '}}' in name template '{t}'")),
            _ => out.push(c),
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        return Err(format!("name template '{t}' produces an invalid file name"));
    }
    if out.chars().any(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
    }) {
        return Err(format!(
            "file name '{out}' contains characters not allowed in file names"
        ));
    }
    Ok(out)
}

/// Natural ("human") ordering: `fig2` < `fig10`, case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut ai), take(&mut bi));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        assert_eq!(render("도 {num}", 3, "a").unwrap(), "도 3");
        assert_eq!(render("도 {n}", 3, "a").unwrap(), "도 3");
        assert_eq!(render("{num:3}", 4, "a").unwrap(), "004");
        assert_eq!(render("FIG{n:02}_{stem}", 7, "scan").unwrap(), "FIG07_scan");
        assert_eq!(render("{stem}", 1, "orig").unwrap(), "orig");
        assert_eq!(render("{{x}}{n}", 1, "a").unwrap(), "{x}1");
        assert!(render("{bad}", 1, "a").is_err());
        assert!(render("a/{n}", 1, "a").is_err());
        assert!(render("{n", 1, "a").is_err());
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["fig10.png", "Fig2.png", "fig1.png", "도 10.jpg", "도 2.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["fig1.png", "Fig2.png", "fig10.png", "도 2.jpg", "도 10.jpg"]
        );
    }
}
