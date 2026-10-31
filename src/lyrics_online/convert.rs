//! 把各家歌词格式统一转成「增强 LRC」文本。
//!
//! QQ 返回 QRC（`[00:24.28]ゆ(24283,183)`），酷狗返回 KRC（`[1000,500]<0,200,0>你`），
//! AMLL 返回 TTML。这里统一成 `[mm:ss.mmm]<mm:ss.mmm|mm:ss.mmm>词…`，
//! 好处是：落盘的就是通用增强 LRC（其他播放器也认），解析层只处理一种格式。

use regex::Regex;
use std::sync::OnceLock;

/// `[mm:ss.mmm]`
pub fn ms_tag(ms: u64) -> String {
    format!(
        "[{:02}:{:02}.{:03}]",
        ms / 60_000,
        (ms % 60_000) / 1000,
        ms % 1000
    )
}

/// `<mm:ss.mmm>`
/// 增强 LRC 的逐字标签：`<起始|结束>`
fn ms_span(st: u64, en: u64) -> String {
    format!(
        "<{:02}:{:02}.{:03}|{:02}:{:02}.{:03}>",
        st / 60_000,
        (st % 60_000) / 1000,
        st % 1000,
        en / 60_000,
        (en % 60_000) / 1000,
        en % 1000
    )
}

/// `[mm:ss.xx]` / `[mm:ss.mmm]` 解析成毫秒
fn parse_tag_ms(s: &str) -> Option<u64> {
    let (m, rest) = s.split_once(':')?;
    let minutes: u64 = m.trim().parse().ok()?;
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    let secs: u64 = sec.trim().parse().ok()?;
    let mut ms = 0u64;
    if !frac.is_empty() {
        let digits: String = frac.chars().take(3).collect();
        let v: u64 = digits.parse().ok()?;
        ms = match digits.len() {
            1 => v * 100,
            2 => v * 10,
            _ => v,
        };
    }
    Some(minutes * 60_000 + secs * 1000 + ms)
}

fn qrc_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\((\d+),(\d+)\)").unwrap())
}

/// 从 QRC XML 里取出 `LyricContent="..."` 的内容；本来就是纯歌词则原样返回。
///
/// QQ 返回的是一段 XML，歌词塞在 `LyricContent` 属性里（值含换行）。
/// 这里扫描到「后面紧跟 `>` 或 `/>`」的引号作为结束，避免被歌词正文里的引号带偏。
pub fn extract_qrc_content(text: &str) -> String {
    let key = "LyricContent=\"";
    let Some(i) = text.find(key) else {
        return text.to_string();
    };
    let rest = &text[i + key.len()..];
    for (k, _) in rest.match_indices('"') {
        let tail = rest[k + 1..].trim_start();
        // 属性值结束后应当是 `>` 或 `/>`
        if tail.starts_with('>') || tail.starts_with('/') {
            return rest[..k].to_string();
        }
    }
    if let Some(k) = rest.rfind('"') {
        if rest[..k].contains('[') {
            return rest[..k].to_string();
        }
    }
    rest.trim_end_matches(['>', '/', ' ', '\n', '\r'])
        .to_string()
}

/// QRC → 增强 LRC。QRC 的逐字形态是 `字(起始ms,时长ms)`。
pub fn qrc_to_lrc(qrc: &str) -> String {
    let qrc = extract_qrc_content(qrc);
    let qrc = qrc.as_str();
    let mut out: Vec<String> = Vec::new();
    for raw in qrc.lines() {
        let line = raw.trim();
        if line.is_empty() || !line.starts_with('[') {
            continue;
        }
        let Some(close) = line.find(']') else {
            continue;
        };
        let head = &line[1..close];
        // QQ 的 QRC 行头有两种：新的 `[起始ms,行时长ms]`，旧的 `[mm:ss.xx]`
        let line_ms = match head.split_once(',') {
            Some((a, b))
                if a.trim().parse::<u64>().is_ok() && b.trim().parse::<u64>().is_ok() =>
            {
                a.trim().parse::<u64>().unwrap_or(0)
            }
            _ => match parse_tag_ms(head) {
                Some(v) => v,
                // [ti:]/[ar:]/[offset:]/[kana:] 等元信息，跳过
                None => continue,
            },
        };
        let body = line[close + 1..].trim_end();
        if body.is_empty() {
            continue;
        }

        let mut words: Vec<(String, u64, u64)> = Vec::new();
        let mut last = 0usize;
        for m in qrc_word_re().captures_iter(body) {
            let start: u64 = m
                .get(1)
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(line_ms);
            let dur: u64 = m
                .get(2)
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(0);
            let whole = m.get(0).unwrap();
            let seg = &body[last..whole.start()];
            if !seg.trim().is_empty() {
                words.push((seg.to_string(), start, start + dur));
            }
            last = whole.end();
        }
        if words.is_empty() {
            out.push(format!("{}{}", ms_tag(line_ms), body));
            continue;
        }
        // 行头只到 10ms（两位小数），逐字有毫秒精度：以首词时间为准
        let head_ms = if words[0].1.abs_diff(line_ms) < 1000 {
            words[0].1
        } else {
            line_ms
        };
        // 尾部没有时间戳的残留文字，归到最后一个词
        let tail = body[last..].trim_end();
        let mut s = ms_tag(head_ms);
        for (text, st, en) in words {
            s.push_str(&ms_span(st, en));
            s.push_str(&text);
        }
        if !tail.trim().is_empty() {
            s.push_str(tail);
        }
        out.push(s);
    }
    out.join("\n")
}

fn krc_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<(\d+),(\d+),-?\d+>").unwrap())
}

/// KRC → 增强 LRC。KRC 的行头是 `[起始ms,时长ms]`，逐字偏移相对行首。
pub fn krc_to_lrc(krc: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in krc.lines() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if !line.starts_with('[') {
            continue;
        }
        let Some(close) = line.find(']') else {
            continue;
        };
        let head = &line[1..close];
        let Some((a, b)) = head.split_once(',') else {
            continue;
        };
        let (Ok(line_ms), Ok(line_dur)) = (a.trim().parse::<u64>(), b.trim().parse::<u64>()) else {
            continue;
        };
        let body = &line[close + 1..];

        // KRC 的文本在标签**之后**：`<偏移,时长,0>歌词文字`
        let matches: Vec<_> = krc_word_re().captures_iter(body).collect();
        let mut words: Vec<(String, u64, u64)> = Vec::new();
        for (i, m) in matches.iter().enumerate() {
            let whole = m.get(0).unwrap();
            let off: u64 = m
                .get(1)
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(0);
            let dur: u64 = m
                .get(2)
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(0);
            let text_end = matches
                .get(i + 1)
                .and_then(|n| n.get(0))
                .map(|g| g.start())
                .unwrap_or(body.len());
            let text = &body[whole.end()..text_end];
            if !text.trim().is_empty() {
                words.push((text.to_string(), line_ms + off, line_ms + off + dur));
            }
        }
        let tail = "";
        if words.is_empty() {
            let text = body.trim();
            if !text.is_empty() {
                out.push(format!("{}{}", ms_tag(line_ms), text));
            }
            continue;
        }
        let mut s = ms_tag(line_ms);
        for (text, st, en) in words {
            let en = if en > st { en } else { line_ms + line_dur };
            s.push_str(&ms_span(st, en));
            s.push_str(&text);
        }
        if !tail.trim().is_empty() {
            s.push_str(tail);
        }
        out.push(s);
    }
    out.join("\n")
}

/// TTML 时间：`24.283`、`24.283s`、`150ms`、`00:00:24.283`
fn parse_ttml_time(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        let mut total = 0f64;
        for p in &parts {
            total = total * 60.0 + p.trim().parse::<f64>().ok()?;
        }
        return Some((total * 1000.0).round().max(0.0) as u64);
    }
    if let Some(v) = s.strip_suffix("ms") {
        return Some(v.trim().parse::<f64>().ok()?.max(0.0) as u64);
    }
    let v = s.strip_suffix('s').unwrap_or(s);
    Some((v.trim().parse::<f64>().ok()? * 1000.0).round().max(0.0) as u64)
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    // 匹配 name="..."/name='...'
    let pat = name;
    let mut from = 0usize;
    while let Some(i) = tag[from..].find(pat) {
        let start = from + i;
        let after = &tag[start + pat.len()..];
        let after = after.trim_start();
        if let Some(rest) = after.strip_prefix('=') {
            let rest = rest.trim_start();
            let (quote, rest) = match rest.chars().next() {
                Some(c @ ('"' | '\'')) => (c, &rest[1..]),
                _ => return None,
            };
            let end = rest.find(quote)?;
            return Some(&rest[..end]);
        }
        from = start + pat.len();
    }
    None
}

/// AMLL 的 TTML 里，翻译 / 音译 / 伴唱用 `ttm:role="x-translation"` 这类额外 span 附在同一行里。
/// 这些不能混进主歌词，否则高亮会串行。
fn is_auxiliary_span(attrs: &str) -> bool {
    attrs.contains("x-translation")
        || attrs.contains("x-roman")
        || attrs.contains("x-ver")
        || attrs.contains("x-bg")
}

/// TTML → 增强 LRC。只处理 AMLL 这类规整的 `<p>` / `<span>` 结构。
pub fn ttml_to_lrc(ttml: &str) -> String {
    let token_re = Regex::new(r"<(/?)(p|span)\b([^>]*)>").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut line_ms: Option<u64> = None;
    let mut words: Vec<(String, u64, u64)> = Vec::new();
    let mut cursor = 0usize;
    // 每个未闭合 span 是否要跳过（翻译/音译/伴唱）
    let mut skip_stack: Vec<bool> = Vec::new();

    fn flush(
        out: &mut Vec<String>,
        line_ms: Option<u64>,
        words: &mut Vec<(String, u64, u64)>,
    ) {
        if let Some(lm) = line_ms {
            if words.iter().any(|w| !w.0.trim().is_empty()) {
                let mut s = ms_tag(lm);
                for (text, st, en) in words.iter() {
                    s.push_str(&ms_span(*st, *en));
                    s.push_str(text);
                }
                out.push(s);
            }
        }
        words.clear();
    }

    for m in token_re.captures_iter(ttml) {
        let whole = m.get(0).unwrap();
        let skipping = skip_stack.last().copied().unwrap_or(false);

        // 标签之间的文本归到当前 span
        if line_ms.is_some() && !skipping {
            let text = unescape(&ttml[cursor..whole.start()]);
            if !text.trim().is_empty() {
                if let Some(last) = words.last_mut() {
                    last.0.push_str(&text);
                } else {
                    let lm = line_ms.unwrap();
                    words.push((text, lm, lm));
                }
            }
        }
        cursor = whole.end();

        let closing = m.get(1).map(|g| g.as_str()) == Some("/");
        let tag = m.get(2).map(|g| g.as_str()).unwrap_or("");
        let attrs = m.get(3).map(|g| g.as_str()).unwrap_or("");

        if tag == "p" {
            if closing {
                flush(&mut out, line_ms, &mut words);
                line_ms = None;
                skip_stack.clear();
            } else {
                line_ms = attr(attrs, "begin").and_then(parse_ttml_time);
                words.clear();
                skip_stack.clear();
            }
            continue;
        }

        // span
        if closing {
            skip_stack.pop();
            continue;
        }
        let skip = is_auxiliary_span(attrs);
        skip_stack.push(skip);
        if skip {
            continue;
        }
        let begin = attr(attrs, "begin").and_then(parse_ttml_time);
        let end = attr(attrs, "end").and_then(parse_ttml_time);
        // <p> 没写 begin 时，用首个 span 的时间作为行时间
        if line_ms.is_none() {
            line_ms = begin;
        }
        let Some(lm) = line_ms else { continue };
        let b = begin.unwrap_or(lm);
        let e = end.unwrap_or(b);
        words.push((String::new(), b, e.max(b)));
    }
    flush(&mut out, line_ms, &mut words);
    out.join("\n")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qrc_words() {
        let qrc = "[ti:x]\n[kana:1ゆ(946,183)]\n[00:24.28]ゆ(24283,183)め(24466,250)\n";
        let lrc = qrc_to_lrc(qrc);
        assert_eq!(lrc, "[00:24.283]<00:24.283|00:24.466>ゆ<00:24.466|00:24.716>め");
    }

    #[test]
    fn qrc_ms_header() {
        // QQ 新版 QRC 行头是 [起始ms,行时长ms]
        let qrc = "[0,267]Lemon - (0,16)米(17,33)津(50,16)\n[946,1406]夢(946,433)な(1379,430)";
        let lrc = qrc_to_lrc(qrc);
        assert!(lrc.starts_with("[00:00.000]<00:00.000|00:00.016>Lemon - "));
        assert!(lrc.contains("[00:00.946]<00:00.946|00:01.379>夢"));
    }

    #[test]
    fn krc_words() {
        let krc = "[id:$00000000]\n[1000,500]<0,200,0>你<200,300,0>好\n";
        let lrc = krc_to_lrc(krc);
        assert_eq!(lrc, "[00:01.000]<00:01.000|00:01.200>你<00:01.200|00:01.500>好");
    }

    #[test]
    fn ttml_words() {
        let t = r#"<tt><body><div><p begin="1.0" end="2.0"><span begin="1.0" end="1.5">你</span><span begin="1.5" end="2.0">好</span></p></div></body></tt>"#;
        let lrc = ttml_to_lrc(t);
        assert_eq!(lrc, "[00:01.000]<00:01.000|00:01.500>你<00:01.500|00:02.000>好");
    }

    #[test]
    fn ttml_entities() {
        let t = r#"<p begin="1.0" end="2.0"><span begin="1.0" end="2.0">A &amp; B</span></p>"#;
        assert!(ttml_to_lrc(t).contains("A & B"));
    }
}
