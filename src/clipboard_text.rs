//! WindowsクリップボードのHTML/RTFを、ログ・翻訳へ渡せるMarkdownへ変換する。

#[derive(Clone, Copy)]
enum ListKind {
    Unordered,
    Ordered(usize),
}

fn extract_cf_html_fragment(data: &[u8]) -> &[u8] {
    fn offset(data: &[u8], key: &[u8]) -> Option<usize> {
        let pos = data.windows(key.len()).position(|w| w == key)? + key.len();
        let end = data[pos..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .map_or(data.len(), |n| pos + n);
        std::str::from_utf8(&data[pos..end]).ok()?.parse().ok()
    }

    if let (Some(start), Some(end)) = (
        offset(data, b"StartFragment:"),
        offset(data, b"EndFragment:"),
    ) && start < end
        && end <= data.len()
    {
        return &data[start..end];
    }
    let start_marker = b"<!--StartFragment-->";
    let end_marker = b"<!--EndFragment-->";
    if let Some(start) = data
        .windows(start_marker.len())
        .position(|w| w == start_marker)
        && let Some(rel_end) = data[start + start_marker.len()..]
            .windows(end_marker.len())
            .position(|w| w == end_marker)
    {
        let begin = start + start_marker.len();
        return &data[begin..begin + rel_end];
    }
    if let (Some(start), Some(end)) = (offset(data, b"StartHTML:"), offset(data, b"EndHTML:"))
        && start < end
        && end <= data.len()
    {
        return &data[start..end];
    }
    data.split(|b| *b == 0).next().unwrap_or(data)
}

fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(ch) = decoded {
            out.push(ch);
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn escape_markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn push_break(out: &mut String, paragraphs: bool) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
    let wanted = if paragraphs { 2 } else { 1 };
    let existing = out.chars().rev().take_while(|c| *c == '\n').count();
    for _ in existing..wanted {
        out.push('\n');
    }
}

fn push_collapsed_text(out: &mut String, text: &str) {
    let decoded = decode_html_entities(text);
    let mut pending_space = false;
    for ch in decoded.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() && !out.ends_with([' ', '\n', '\t']) {
            out.push(' ');
        }
        pending_space = false;
        out.push_str(&escape_markdown_text(&ch.to_string()));
    }
    if pending_space && !out.is_empty() && !out.ends_with([' ', '\n', '\t']) {
        out.push(' ');
    }
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(pos) = rest.to_ascii_lowercase().find(name) {
        rest = &rest[pos + name.len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('=') {
            continue;
        }
        let value = trimmed[1..].trim_start();
        let raw = if let Some(v) = value.strip_prefix('"') {
            let end = v.find('"')?;
            &v[..end]
        } else if let Some(v) = value.strip_prefix('\'') {
            let end = v.find('\'')?;
            &v[..end]
        } else {
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            &value[..end]
        };
        return Some(decode_html_entities(raw));
    }
    None
}

fn finish_markdown(mut text: String) -> String {
    text = text.replace("\r\n", "\n").replace('\r', "\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim_matches([' ', '\t', '\n']).to_string()
}

pub fn html_to_markdown(data: &[u8]) -> Option<String> {
    let html = String::from_utf8_lossy(extract_cf_html_fragment(data));
    let mut out = String::new();
    let mut rest = html.as_ref();
    let mut lists: Vec<ListKind> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let mut in_pre = false;

    while let Some(start) = rest.find('<') {
        if in_pre {
            out.push_str(&decode_html_entities(&rest[..start]));
        } else {
            push_collapsed_text(&mut out, &rest[..start]);
        }
        let Some(end) = rest[start..].find('>') else {
            push_collapsed_text(&mut out, &rest[start..]);
            rest = "";
            break;
        };
        let raw = rest[start + 1..start + end].trim();
        rest = &rest[start + end + 1..];
        if raw.starts_with('!') || raw.starts_with('?') {
            continue;
        }
        let closing = raw.starts_with('/');
        let body = raw.trim_start_matches('/').trim_start();
        let name = body
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match (closing, name.as_str()) {
            (false, "br") => push_break(&mut out, false),
            (false, "p" | "div" | "section" | "article")
            | (true, "p" | "div" | "section" | "article") => push_break(&mut out, true),
            (false, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                push_break(&mut out, true);
                let level = name[1..].parse::<usize>().unwrap_or(1);
                out.push_str(&"#".repeat(level));
                out.push(' ');
            }
            (true, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => push_break(&mut out, true),
            (false, "strong" | "b") | (true, "strong" | "b") => out.push_str("**"),
            (false, "em" | "i") | (true, "em" | "i") => out.push('*'),
            (false, "s" | "strike" | "del") | (true, "s" | "strike" | "del") => out.push_str("~~"),
            (false, "u") => out.push_str("<u>"),
            (true, "u") => out.push_str("</u>"),
            (false, "blockquote") => {
                push_break(&mut out, true);
                out.push_str("> ");
            }
            (true, "blockquote") => push_break(&mut out, true),
            (false, "ul") => lists.push(ListKind::Unordered),
            (false, "ol") => lists.push(ListKind::Ordered(1)),
            (true, "ul" | "ol") => {
                lists.pop();
                push_break(&mut out, true);
            }
            (false, "li") => {
                push_break(&mut out, false);
                out.push_str(&"  ".repeat(lists.len().saturating_sub(1)));
                match lists.last_mut() {
                    Some(ListKind::Ordered(n)) => {
                        out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => out.push_str("- "),
                }
            }
            (true, "li") => push_break(&mut out, false),
            (false, "a") => {
                out.push('[');
                links.push(attr(body, "href").unwrap_or_default());
            }
            (true, "a") => {
                let url = links.pop().unwrap_or_default().replace(')', "\\)");
                out.push_str("](");
                out.push_str(&url);
                out.push(')');
            }
            (false, "pre") => {
                push_break(&mut out, true);
                out.push_str("```\n");
                in_pre = true;
            }
            (true, "pre") => {
                push_break(&mut out, false);
                out.push_str("```\n\n");
                in_pre = false;
            }
            (false, "code") if !in_pre => out.push('`'),
            (true, "code") if !in_pre => out.push('`'),
            (false, "tr") => push_break(&mut out, false),
            (true, "tr") => {
                out.push('|');
                push_break(&mut out, false);
            }
            (false, "td" | "th") => out.push_str("| "),
            (true, "td" | "th") => out.push(' '),
            (false, "img") => {
                let alt = attr(body, "alt").unwrap_or_default();
                if !alt.is_empty() {
                    out.push_str(&format!("![{}]", escape_markdown_text(&alt)));
                }
            }
            _ => {}
        }
    }
    if in_pre {
        out.push_str(&decode_html_entities(rest));
        push_break(&mut out, false);
        out.push_str("```");
    } else {
        push_collapsed_text(&mut out, rest);
    }
    let result = finish_markdown(out);
    (!result.is_empty()).then_some(result)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct RtfState {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    skip: bool,
    group_start: bool,
    uc: usize,
}

fn transition_rtf_style(out: &mut String, from: RtfState, to: RtfState) {
    if from.strike && !to.strike {
        out.push_str("~~");
    }
    if from.underline && !to.underline {
        out.push_str("</u>");
    }
    if from.italic && !to.italic {
        out.push('*');
    }
    if from.bold && !to.bold {
        out.push_str("**");
    }
    if !from.bold && to.bold {
        out.push_str("**");
    }
    if !from.italic && to.italic {
        out.push('*');
    }
    if !from.underline && to.underline {
        out.push_str("<u>");
    }
    if !from.strike && to.strike {
        out.push_str("~~");
    }
}

fn cp1252(byte: u8) -> char {
    const EXT: [char; 32] = [
        '€', '\u{0081}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{008d}', 'Ž',
        '\u{008f}', '\u{0090}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
        '\u{009d}', 'ž', 'Ÿ',
    ];
    if (0x80..=0x9f).contains(&byte) {
        EXT[(byte - 0x80) as usize]
    } else {
        byte as char
    }
}

fn push_utf16_unit(out: &mut String, pending: &mut Option<u16>, unit: u16) {
    if (0xd800..=0xdbff).contains(&unit) {
        if pending.replace(unit).is_some() {
            out.push('\u{fffd}');
        }
    } else if (0xdc00..=0xdfff).contains(&unit) {
        if let Some(high) = pending.take() {
            let value = 0x10000 + (((high - 0xd800) as u32) << 10) + (unit - 0xdc00) as u32;
            out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
        } else {
            out.push('\u{fffd}');
        }
    } else {
        if pending.take().is_some() {
            out.push('\u{fffd}');
        }
        out.push(char::from_u32(unit as u32).unwrap_or('\u{fffd}'));
    }
}

pub fn rtf_to_markdown(data: &[u8]) -> Option<String> {
    if !data.starts_with(b"{\\rtf") {
        return None;
    }
    let mut out = String::new();
    let mut stack = vec![RtfState {
        group_start: true,
        uc: 1,
        ..Default::default()
    }];
    let mut i = 0usize;
    let mut fallback = 0usize;
    let mut pending_utf16 = None;
    while i < data.len() {
        let mut state = *stack.last().unwrap_or(&RtfState::default());
        match data[i] {
            b'{' => {
                state.group_start = true;
                stack.push(state);
                i += 1;
            }
            b'}' => {
                if stack.len() > 1 {
                    let from = stack.pop().unwrap_or_default();
                    let to = *stack.last().unwrap_or(&RtfState::default());
                    if !from.skip && !to.skip {
                        transition_rtf_style(&mut out, from, to);
                    }
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= data.len() {
                    break;
                }
                if matches!(data[i], b'\\' | b'{' | b'}') {
                    if fallback > 0 {
                        fallback -= 1;
                    } else if !state.skip {
                        out.push(data[i] as char);
                    }
                    i += 1;
                    continue;
                }
                if data[i] == b'\'' && i + 2 < data.len() {
                    if let Ok(hex) = std::str::from_utf8(&data[i + 1..i + 3])
                        && let Ok(byte) = u8::from_str_radix(hex, 16)
                    {
                        if fallback > 0 {
                            fallback -= 1;
                        } else if !state.skip {
                            out.push(cp1252(byte));
                        }
                    }
                    i += 3;
                    continue;
                }
                if data[i] == b'*' {
                    if let Some(last) = stack.last_mut() {
                        last.skip = true;
                        last.group_start = false;
                    }
                    i += 1;
                    continue;
                }
                if !data[i].is_ascii_alphabetic() {
                    let symbol = data[i];
                    if !state.skip && fallback == 0 {
                        match symbol {
                            b'~' => out.push(' '),
                            b'_' => out.push('-'),
                            b'-' => {}
                            _ => {}
                        }
                    }
                    i += 1;
                    continue;
                }
                let word_start = i;
                while i < data.len() && data[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = std::str::from_utf8(&data[word_start..i]).unwrap_or("");
                let mut sign = 1i32;
                if i < data.len() && data[i] == b'-' {
                    sign = -1;
                    i += 1;
                }
                let num_start = i;
                while i < data.len() && data[i].is_ascii_digit() {
                    i += 1;
                }
                let number = (num_start < i)
                    .then(|| {
                        std::str::from_utf8(&data[num_start..i])
                            .ok()?
                            .parse::<i32>()
                            .ok()
                    })
                    .flatten()
                    .map(|n| n * sign);
                if i < data.len() && data[i] == b' ' {
                    i += 1;
                }
                let destinations = [
                    "fonttbl",
                    "colortbl",
                    "stylesheet",
                    "info",
                    "pict",
                    "object",
                    "header",
                    "footer",
                    "generator",
                    "xmlnstbl",
                    "listtable",
                    "listoverridetable",
                ];
                if state.group_start && destinations.contains(&word) {
                    if let Some(last) = stack.last_mut() {
                        last.skip = true;
                        last.group_start = false;
                    }
                    continue;
                }
                if let Some(last) = stack.last_mut() {
                    last.group_start = false;
                }
                state = *stack.last().unwrap_or(&state);
                if state.skip {
                    continue;
                }
                match word {
                    "b" | "i" | "ul" | "strike" => {
                        let mut next = state;
                        let enabled = number != Some(0);
                        match word {
                            "b" => next.bold = enabled,
                            "i" => next.italic = enabled,
                            "ul" => next.underline = enabled,
                            "strike" => next.strike = enabled,
                            _ => {}
                        }
                        transition_rtf_style(&mut out, state, next);
                        *stack.last_mut().unwrap() = next;
                    }
                    "ulnone" => {
                        let mut next = state;
                        next.underline = false;
                        transition_rtf_style(&mut out, state, next);
                        *stack.last_mut().unwrap() = next;
                    }
                    "plain" => {
                        let mut next = state;
                        next.bold = false;
                        next.italic = false;
                        next.underline = false;
                        next.strike = false;
                        transition_rtf_style(&mut out, state, next);
                        *stack.last_mut().unwrap() = next;
                    }
                    "par" => push_break(&mut out, true),
                    "line" => push_break(&mut out, false),
                    "tab" => out.push('\t'),
                    "bullet" => out.push_str("- "),
                    "emdash" => out.push('—'),
                    "endash" => out.push('–'),
                    "lquote" => out.push('‘'),
                    "rquote" => out.push('’'),
                    "ldblquote" => out.push('“'),
                    "rdblquote" => out.push('”'),
                    "uc" => {
                        if let Some(n) = number
                            && let Some(last) = stack.last_mut()
                        {
                            last.uc = n.max(0) as usize;
                        }
                    }
                    "u" => {
                        if let Some(n) = number {
                            let unit = if n < 0 { (n + 65_536) as u16 } else { n as u16 };
                            push_utf16_unit(&mut out, &mut pending_utf16, unit);
                            fallback = state.uc;
                        }
                    }
                    "bin" => {
                        i = (i + number.unwrap_or(0).max(0) as usize).min(data.len());
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            byte => {
                if fallback > 0 {
                    fallback -= 1;
                } else if !state.skip && byte != 0 {
                    out.push(cp1252(byte));
                }
                if let Some(last) = stack.last_mut() {
                    last.group_start = false;
                }
                i += 1;
            }
        }
    }
    if pending_utf16.is_some() {
        out.push('\u{fffd}');
    }
    if let Some(state) = stack.last().copied() {
        transition_rtf_style(&mut out, state, RtfState::default());
    }
    let result = finish_markdown(out);
    (!result.is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_htmlのfragmentをmarkdownへ変換する() {
        let fragment = b"<h2>Title</h2><p>Hello <strong>world</strong> &amp; <a href=\"https://example.com\">link</a>.</p><ul><li>one</li><li>two</li></ul>";
        let blank = "Version:1.0\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let start = blank.len();
        let end = start + fragment.len();
        let header = format!(
            "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{start:010}\r\nEndFragment:{end:010}\r\n"
        );
        let mut data = header.into_bytes();
        data.extend_from_slice(fragment);

        assert_eq!(
            html_to_markdown(&data).as_deref(),
            Some("## Title\n\nHello **world** & [link](https://example.com).\n\n- one\n- two")
        );
    }

    #[test]
    fn htmlの引用とコードを保持する() {
        let html = b"<!--StartFragment--><blockquote>A &lt; B</blockquote><pre>x\n y</pre><!--EndFragment-->";
        assert_eq!(
            html_to_markdown(html).as_deref(),
            Some("> A \\< B\n\n```\nx\n y\n```")
        );
    }

    #[test]
    fn rtfのunicodeと基本書式をmarkdownへ変換する() {
        let rtf =
            br#"{\rtf1\ansi\uc1 Plain \b bold\b0  \i italic\i0 \par \u26085?\u26412?\u35486?}"#;
        assert_eq!(
            rtf_to_markdown(rtf).as_deref(),
            Some("Plain **bold** *italic*\n\n日本語")
        );
    }

    #[test]
    fn rtfのメタデータを本文へ混ぜない() {
        let rtf = br#"{\rtf1{\fonttbl{\f0 Arial;}}{\info{\author Secret}}Body}"#;
        assert_eq!(rtf_to_markdown(rtf).as_deref(), Some("Body"));
    }
}
