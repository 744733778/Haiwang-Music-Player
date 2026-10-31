//! 歌词匹配评分。
//!
//! 算法移植自 chthollyphile/folia-major 的 `matchScore.ts`：
//! 标题 45 / 歌手 25 / 专辑 30 权重 + 时长倍率；标题未命中或歌手专辑都不可信时封顶 74。

use crate::library::Song;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

fn version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(instrumental|inst|off\s*vocal|karaoke|remix|mix|version|ver\.?|cover|live|edit|arrange|伴奏|カラオケ|インスト|リミックス|remaster|remastered)").unwrap()
    })
}
fn feat_bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?iu)[\(\[（【]\s*(feat|featuring|ft)\.?\s+[^\)\]）】]+[\)\]）】]").unwrap())
}
fn feat_tail_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?iu)\b(feat|featuring|ft)\.?\s+.+$").unwrap())
}
fn bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\(\[（【]([^\)\]）】]+)[\)\]）】]").unwrap())
}
fn artist_split_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)[,，&、/]|feat\.?|ft\.?|featuring|与").unwrap())
}

/// 去掉标点/符号与多余空白，保留各语言的字母数字
pub fn normalize_match_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        } else if ch.is_whitespace() {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_title(s: &str) -> String {
    let mut t = feat_tail_re().replace_all(s, "").to_string();
    t = feat_bracket_re().replace_all(&t, "").to_string();
    let t = bracket_re()
        .replace_all(&t, |caps: &regex::Captures<'_>| {
            let inner = &caps[1];
            if version_re().is_match(inner) {
                caps[0].to_string()
            } else {
                String::new()
            }
        })
        .to_string();
    normalize_match_text(&t)
}

/// 「歌手 - 标题」这种把歌手重复写进标题的情况，只留标题
pub fn normalize_song_title(title: &str, artist: &str) -> String {
    let t = title.trim();
    let a = artist.trim();
    if t.is_empty() || a.is_empty() {
        return t.to_string();
    }
    if let Some((left, right)) = t.split_once(" - ") {
        let ln: String = normalize_match_text(left)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let an: String = normalize_match_text(a)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if !ln.is_empty() && ln == an {
            return right.trim().to_string();
        }
    }
    t.to_string()
}

/// 去掉歌名末尾的括号说明：`Idol (TV Size)` → `Idol`
pub fn strip_title_suffix(title: &str) -> String {
    let re = strip_suffix_re();
    let t = re.replace(title, "").trim().to_string();
    if t.is_empty() {
        title.trim().to_string()
    } else {
        t
    }
}
fn strip_suffix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:\s*[(\[（【][^)\]）】]*[)\]）】])+\s*$").unwrap())
}

fn similarity(a: &str, b: &str, norm: fn(&str) -> String) -> f64 {
    let n1 = norm(a);
    let n2 = norm(b);
    if n1.is_empty() || n2.is_empty() {
        return 0.0;
    }
    if n1 == n2 {
        return 1.0;
    }
    if n1.contains(&n2) || n2.contains(&n1) {
        let l1 = n1.chars().count();
        let l2 = n2.chars().count();
        return l1.min(l2) as f64 / l1.max(l2) as f64;
    }
    let s1: HashSet<char> = n1.chars().collect();
    let s2: HashSet<char> = n2.chars().collect();
    let inter = s1.intersection(&s2).count();
    let union = s1.union(&s2).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

fn split_artists(s: &str) -> Vec<String> {
    artist_split_re()
        .split(s)
        .map(normalize_match_text)
        .filter(|x| !x.is_empty())
        .collect()
}

/// 按常见分隔符拆歌手名，保留原始字符（给 QQ 的 singerName 之类的接口用）
pub fn split_artist_names(s: &str) -> Vec<String> {
    artist_split_re()
        .split(s)
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn artist_similarity(a: &str, b: &str) -> f64 {
    let ta = split_artists(a);
    let tb = split_artists(b);
    if ta.is_empty() || tb.is_empty() {
        return similarity(a, b, normalize_match_text);
    }
    let mut matched = 0usize;
    for x in &ta {
        for y in &tb {
            if x == y
                || (x.chars().count() >= 3 && y.contains(x.as_str()))
                || (y.chars().count() >= 3 && x.contains(y.as_str()))
            {
                matched += 1;
                break;
            }
        }
    }
    let token = matched as f64 / ta.len().max(tb.len()) as f64;
    let main_matched = match (ta.first(), tb.first()) {
        (Some(x), Some(y)) => {
            x == y
                || (x.chars().count() >= 3 && y.contains(x.as_str()))
                || (y.chars().count() >= 3 && x.contains(y.as_str()))
        }
        _ => false,
    };
    let base = if main_matched { token.max(0.7) } else { token };
    base.max(similarity(a, b, normalize_match_text))
}


fn duration_multiplier(a: u64, b: u64) -> f64 {
    if a == 0 || b == 0 {
        // 一方没有时长（AMLL 搜索就不给），按未知处理，轻微折扣
        return 0.9;
    }
    let d = a.abs_diff(b);
    if d <= 1000 {
        1.0
    } else if d <= 3000 {
        0.95
    } else if d <= 5000 {
        0.75
    } else if d <= 10000 {
        0.35
    } else {
        0.1
    }
}

/// 给一个候选打分（0-100）
pub fn score(
    song: &Song,
    cand_name: &str,
    cand_artist: &str,
    cand_album: &str,
    cand_duration_ms: u64,
) -> i64 {
    let title_sim = similarity(&song.title, cand_name, normalize_title);
    let title_score = title_sim * 45.0;

    let artist_sim = if song.artist.trim().is_empty() {
        1.0
    } else {
        artist_similarity(&song.artist, cand_artist)
    };
    let artist_score = artist_sim * 25.0;

    let has_ta = !song.album.trim().is_empty();
    let has_ca = !cand_album.trim().is_empty() && !cand_album.trim().eq_ignore_ascii_case("unknown album");
    let album_sim = if has_ta && has_ca {
        Some(similarity(&song.album, cand_album, normalize_title))
    } else {
        None
    };
    let album_score = match album_sim {
        None => {
            if has_ta {
                0.0
            } else {
                30.0
            }
        }
        Some(s) => s * 30.0,
    };

    let identity = title_score + artist_score + album_score;
    let title_matched = title_sim >= 0.65;
    let artist_matched = song.artist.trim().is_empty() || artist_sim >= 0.5;
    let album_matched = if !has_ta {
        None
    } else if has_ca {
        Some(album_sim.unwrap_or(0.0) >= 0.65)
    } else {
        None
    };
    // 歌手完全对不上（本地有歌手信息时）是最强的负面信号：
    // 这是翻唱/同名不同歌的典型特征，不能靠专辑名蒙混过关。
    let artist_mismatch = !song.artist.trim().is_empty() && artist_sim < 0.35;
    let reliable = artist_matched || album_matched == Some(true);
    let capped = if artist_mismatch {
        identity.min(68.0)
    } else if !title_matched || !reliable {
        identity.min(74.0)
    } else {
        identity
    };
    let s = capped * duration_multiplier(song.duration_ms, cand_duration_ms);
    s.round().clamp(0.0, 100.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn song(title: &str, artist: &str, album: &str, dur: u64) -> Song {
        Song {
            path: PathBuf::from("x.mp3"),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms: dur,
        }
    }

    #[test]
    fn exact_match_scores_high() {
        let s = song("晴天", "周杰伦", "叶惠美", 269000);
        assert!(score(&s, "晴天", "周杰伦", "叶惠美", 269000) >= 95);
    }

    #[test]
    fn wrong_artist_is_penalized() {
        let s = song("晴天", "周杰伦", "叶惠美", 269000);
        assert!(score(&s, "晴天", "某某", "别的", 269000) < 60);
    }

    #[test]
    fn cover_with_suffix_is_lower() {
        let s = song("晴天", "周杰伦", "叶惠美", 269000);
        let cover = score(&s, "晴天 (Live)", "周杰伦", "叶惠美", 269000);
        let orig = score(&s, "晴天", "周杰伦", "叶惠美", 269000);
        assert!(cover <= orig);
    }

    #[test]
    fn normalize_keeps_cjk() {
        assert_eq!(normalize_match_text("晴天 (Live)!"), "晴天 live");
    }

    #[test]
    fn strip_suffix_works() {
        assert_eq!(strip_title_suffix("Idol (TV Size)"), "Idol");
    }
}
