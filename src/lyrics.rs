use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct Word {
    pub time: u64,
    pub text: String,
    pub end: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum LyricLine {
    Simple {
        time: u64,
        text: String,
        translation: Option<String>,
    },
    PerWord {
        time: u64,
        words: Vec<Word>,
        translation: Option<String>,
    },
}

impl LyricLine {
    pub fn time(&self) -> u64 {
        match self {
            Self::Simple { time, .. } => *time,
            Self::PerWord { time, .. } => *time,
        }
    }

    pub fn text(&self) -> String {
        match self {
            Self::Simple { text, .. } => text.clone(),
            Self::PerWord { words, .. } => words.iter().map(|w| w.text.as_str()).collect(),
        }
    }

    pub fn translation(&self) -> Option<&str> {
        match self {
            Self::Simple { translation, .. } => translation.as_deref(),
            Self::PerWord { translation, .. } => translation.as_deref(),
        }
    }

    pub fn set_translation(&mut self, t: Option<String>) {
        match self {
            Self::Simple { translation, .. } => *translation = t,
            Self::PerWord { translation, .. } => *translation = t,
        }
    }

    /// 已唱部分占整行的字符比例（0..1）。逐字歌词用词时间轴，普通行返回 0/1。
    pub fn sung_fraction(&self, pos_ms: u64, line_end_ms: u64) -> f64 {
        match self {
            Self::Simple { time, .. } => {
                if pos_ms < *time {
                    0.0
                } else {
                    1.0
                }
            }
            Self::PerWord { time, words, .. } => {
                if words.is_empty() {
                    return if pos_ms >= *time { 1.0 } else { 0.0 };
                }
                let total: usize = words.iter().map(|w| w.text.chars().count()).sum();
                if total == 0 {
                    return 1.0;
                }
                let mut sung = 0.0f64;
                for (i, w) in words.iter().enumerate() {
                    let wlen = w.text.chars().count() as f64;
                    let wend = w
                        .end
                        .filter(|e| *e > w.time)
                        .or_else(|| words.get(i + 1).map(|n| n.time))
                        .unwrap_or(line_end_ms)
                        .max(w.time);
                    if pos_ms >= wend {
                        sung += wlen;
                    } else if pos_ms > w.time {
                        let span = (wend - w.time).max(1) as f64;
                        sung += wlen * (((pos_ms - w.time) as f64) / span).clamp(0.0, 1.0);
                    }
                }
                (sung / total as f64).clamp(0.0, 1.0)
            }
        }
    }
}

fn enhanced_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"<(\d+:){1,2}\d+(\.\d*)?(?:\|(\d+:){1,2}\d+(\.\d*)?)?>").unwrap()
    })
}

fn verbatim_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[(\d+:){1,2}\d+(\.\d*)?\]").unwrap())
}

fn parse_time(s: &str, offset_ms: i64) -> Option<u64> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let parts: Vec<&str> = body.split(':').collect();
    let minutes: f64 = match parts.len() {
        2 => parts[0].trim().parse().ok()?,
        3 => parts[0].trim().parse::<f64>().ok()? * 60.0 + parts[1].trim().parse::<f64>().ok()?,
        _ => return None,
    };
    let secs: f64 = parts.last()?.trim().parse().ok()?;
    let mut ms = ((minutes * 60.0 + secs) * 1000.0).round() as i64;
    if neg {
        ms = -ms;
    }
    // offset 为正表示歌词更早
    ms -= offset_ms;
    Some(ms.max(0) as u64)
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(';').map(|e| e + 1);
        if let Some(end) = end {
            let ent = &tail[1..end - 1];
            let decoded = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => {
                    if let Some(num) = ent.strip_prefix('#') {
                        if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                        } else {
                            num.parse::<u32>().ok().and_then(char::from_u32)
                        }
                    } else {
                        None
                    }
                }
            };
            if let Some(c) = decoded {
                out.push(c);
                rest = &tail[end..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

fn parse_enhanced(time: u64, offset: i64, content: &str) -> LyricLine {
    let matches: Vec<_> = enhanced_re().find_iter(content).collect();
    if matches.is_empty() {
        return LyricLine::Simple {
            time,
            text: decode_entities(content).trim().to_string(),
            translation: None,
        };
    }

    struct Tag {
        start: usize,
        end: usize,
        t: u64,
        t_end: Option<u64>,
    }

    let tags: Vec<Tag> = matches
        .iter()
        .map(|m| {
            let inner = &content[m.start() + 1..m.end() - 1];
            let parts: Vec<&str> = inner.split('|').collect();
            Tag {
                start: m.start(),
                end: m.end(),
                t: parse_time(parts[0], offset).unwrap_or(time),
                t_end: parts.get(1).and_then(|p| parse_time(p, offset)),
            }
        })
        .collect();

    let mut words: Vec<Word> = Vec::new();
    let mut last = time;

    let mut push = |st: u64, en: Option<u64>, text: &str| {
        let decoded = decode_entities(text);
        if decoded.trim().is_empty() {
            return;
        }
        let st = st.max(last);
        words.push(Word {
            time: st,
            text: decoded,
            end: en.filter(|e| *e > st),
        });
        last = st;
    };

    push(time, Some(tags[0].t), &content[..tags[0].start]);
    for (i, tag) in tags.iter().enumerate() {
        let text_start = tag.end;
        let text_end = tags.get(i + 1).map(|n| n.start).unwrap_or(content.len());
        let en = tag.t_end.or_else(|| tags.get(i + 1).map(|n| n.t));
        push(tag.t, en, &content[text_start..text_end]);
    }

    if words.is_empty() {
        return LyricLine::Simple {
            time,
            text: decode_entities(content).trim().to_string(),
            translation: None,
        };
    }
    LyricLine::PerWord {
        time,
        words,
        translation: None,
    }
}

fn parse_verbatim(time: u64, offset: i64, content: &str) -> LyricLine {
    let mut words: Vec<Word> = Vec::new();
    let mut current = time;
    let mut last_start = 0usize;

    for m in verbatim_re().find_iter(content) {
        let text = decode_entities(&content[last_start..m.start()]);
        if !text.is_empty() {
            words.push(Word {
                time: current,
                text,
                end: None,
            });
        }
        if let Some(t) = parse_time(&content[m.start() + 1..m.end() - 1], offset) {
            if t > current {
                current = t;
            }
        }
        last_start = m.end();
    }
    let tail = decode_entities(&content[last_start..]);
    if !tail.is_empty() || words.is_empty() {
        words.push(Word {
            time: current,
            text: tail,
            end: None,
        });
    }
    LyricLine::PerWord {
        time,
        words,
        translation: None,
    }
}

fn parse_content(time: u64, offset: i64, content: &str) -> LyricLine {
    if enhanced_re().is_match(content) {
        parse_enhanced(time, offset, content)
    } else if verbatim_re().is_match(content) {
        parse_verbatim(time, offset, content)
    } else {
        LyricLine::Simple {
            time,
            text: decode_entities(content).trim().to_string(),
            translation: None,
        }
    }
}

/// 解析 LRC 文本（标准 / 增强逐字 / 逐字 / offset / 多时间戳）
pub fn parse_str(data: &str) -> Vec<LyricLine> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut offset: i64 = 0;

    for raw in data.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        // [offset:±ms] / [ti:] 等纯标签行
        if line.starts_with('[') {
            if let Some(close) = line.find(']') {
                let tag = &line[1..close];
                if let Some(v) = tag.strip_prefix("offset:") {
                    if let Ok(n) = v.trim().parse::<i64>() {
                        offset = n;
                    }
                    continue;
                }
            }
        }

        let mut idx = 0usize;
        let mut times: Vec<u64> = Vec::new();
        while line[idx..].starts_with('[') {
            let Some(rel) = line[idx..].find(']') else { break };
            let close = idx + rel;
            let inner = &line[idx + 1..close];
            if let Some(t) = parse_time(inner, offset) {
                times.push(t);
            } else if enhanced_re().is_match(inner) || verbatim_re().is_match(inner) {
                break;
            }
            idx = close + 1;
            if idx >= line.len() {
                break;
            }
        }

        if times.is_empty() {
            continue;
        }
        let content = &line[idx..];
        if content.trim().is_empty() {
            continue;
        }
        for t in times {
            lines.push(parse_content(t, offset, content));
        }
    }

    lines.sort_by_key(|l| l.time());
    lines
}

/// 把翻译歌词按时间合并进主歌词
pub fn merge_translation(main: &mut [LyricLine], trans: &[LyricLine]) {
    if trans.is_empty() {
        return;
    }
    for line in main.iter_mut() {
        let t = line.time();
        let best = trans
            .iter()
            .min_by_key(|x| x.time().abs_diff(t))
            .filter(|x| x.time().abs_diff(t) <= 200 && !x.text().trim().is_empty());
        if let Some(b) = best {
            if line.translation().is_none() {
                line.set_translation(Some(b.text()));
            }
        }
    }
}

pub fn decode_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(&bytes[2..]).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(&bytes[2..]).0.into_owned();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let (s, _, _) = encoding_rs::GBK.decode(bytes);
    s.into_owned()
}

pub fn load_lrc_file(path: &std::path::Path) -> Option<Vec<LyricLine>> {
    let bytes = std::fs::read(path).ok()?;
    let text = decode_bytes(&bytes);
    let lines = parse_str(&text);
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple() {
        let l = parse_str("[00:01.00]hello\n[00:02.50]world");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].time(), 1000);
        assert_eq!(l[1].text(), "world");
    }

    #[test]
    fn offset_and_multi_ts() {
        let l = parse_str("[offset:500]\n[00:01.00][00:03.00]hi");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].time(), 500);
        assert_eq!(l[1].time(), 2500);
    }

    #[test]
    fn enhanced_word() {
        let l = parse_str("[00:10.00]<00:10.00|00:10.50>你<00:10.50|00:11.00>好");
        match &l[0] {
            LyricLine::PerWord { words, .. } => {
                assert_eq!(words.len(), 2);
                assert_eq!(words[0].text, "你");
                assert_eq!(words[0].end, Some(10500));
            }
            _ => panic!("expected per-word"),
        }
    }

    #[test]
    fn verbatim_word() {
        let l = parse_str("[00:10.00]你[00:10.50]好");
        match &l[0] {
            LyricLine::PerWord { words, .. } => {
                assert_eq!(words.len(), 2);
                assert_eq!(words[1].time, 10500);
            }
            _ => panic!("expected per-word"),
        }
    }

    #[test]
    fn translation_merge() {
        let mut main = parse_str("[00:01.00]hello");
        let trans = parse_str("[00:01.05]你好");
        merge_translation(&mut main, &trans);
        assert_eq!(main[0].translation(), Some("你好"));
    }
}
