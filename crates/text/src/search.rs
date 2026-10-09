//! Search and replace (§6.1).
//!
//! - Patterns compile with `regex-automata`'s meta engine; a literal search is
//!   an escaped pattern, which the engine runs as a `memmem` scan.
//! - A full search splits the document into 16 MiB chunks, searched in
//!   parallel on the scheduler. Each chunk reads 64 KiB past its end (plus a
//!   few bytes before its start, for `\b` and `^`), and only keeps matches
//!   that start inside it. A match that runs into the end of that window is
//!   re-run on a longer window (up to [`MAX_MATCH`]). Chunks merge in order;
//!   when a match from one chunk overlaps the next, the next chunk is
//!   re-searched from where that match ended, so the result is exactly the
//!   sequential leftmost-first result.
//! - Results stream in batches from the first hit and stop being stored past
//!   [`MAX_STORED`] (the count continues).
//! - Empty matches are skipped.

use std::io;
use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use quire_core::scheduler::{CancelToken, JobSpec, Priority, Scheduler};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use regex_automata::{Anchored, Input};
use serde::{Deserialize, Serialize};

use crate::buffer::Snapshot;

// Tests and fuzzing use tiny sizes so chunk seams, overlap and long matches
// are hit.
#[cfg(not(any(test, fuzzing)))]
pub const CHUNK: u64 = 16 << 20;
#[cfg(all(test, not(fuzzing)))]
pub const CHUNK: u64 = 4096;
#[cfg(fuzzing)]
pub const CHUNK: u64 = 64;
#[cfg(not(any(test, fuzzing)))]
pub const OVERLAP: u64 = 64 * 1024;
#[cfg(all(test, not(fuzzing)))]
pub const OVERLAP: u64 = 64;
#[cfg(fuzzing)]
pub const OVERLAP: u64 = 16;
/// Look-behind context read before each chunk.
const BEHIND: u64 = 4;
/// Longest match found exactly; longer ones are cut here.
#[cfg(not(any(test, fuzzing)))]
pub const MAX_MATCH: u64 = 16 << 20;
#[cfg(any(test, fuzzing))]
pub const MAX_MATCH: u64 = 100_000;
/// Matches kept in memory (16 bytes each).
pub const MAX_STORED: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Matcher {
    pub query: Query,
    re: Arc<Regex>,
}

impl Matcher {
    pub fn new(query: &Query) -> Result<Self, String> {
        if query.pattern.is_empty() {
            return Err("empty pattern".into());
        }
        let mut pat = if query.regex {
            query.pattern.clone()
        } else {
            regex_syntax::escape(&query.pattern)
        };
        if query.whole_word {
            pat = format!(r"\b(?:{pat})\b");
        }
        let re = Regex::builder()
            .syntax(
                syntax::Config::new()
                    .case_insensitive(!query.case_sensitive)
                    .multi_line(true)
                    .crlf(true),
            )
            .build(&pat)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            query: query.clone(),
            re: Arc::new(re),
        })
    }

    pub fn regex(&self) -> &Regex {
        &self.re
    }

    /// Matches in `hay[span]` that start before `start_limit` (all
    /// positions relative to `hay`). The second slot is set when a match
    /// ran into the end of a `hay` that is `cut` short of the document's
    /// end, so it may be longer.
    fn find_in(
        &self,
        hay: &[u8],
        span: Range<usize>,
        start_limit: usize,
        cut: bool,
    ) -> (Vec<Range<usize>>, Option<usize>) {
        let mut out = Vec::new();
        let mut at = span.start;
        while at <= span.end {
            let input = Input::new(hay).span(at..span.end);
            let Some(m) = self.re.search(&input) else {
                break;
            };
            if m.start() >= start_limit {
                break;
            }
            if cut && m.end() == hay.len() {
                return (out, Some(m.start()));
            }
            if m.is_empty() {
                at = m.end() + 1;
                continue;
            }
            out.push(m.start()..m.end());
            at = m.end();
        }
        (out, None)
    }
}

/// Searches `range` of `snap` sequentially from `from`. Used for chunks,
/// small documents and find-next.
pub fn search_range(
    snap: &Snapshot,
    m: &Matcher,
    from: u64,
    to: u64,
    limit: usize,
) -> io::Result<Vec<(u64, u64)>> {
    let len = snap.len();
    let to = to.min(len);
    let mut out = Vec::new();
    let mut at = from.min(to);
    while at < to && out.len() < limit {
        let chunk_end = (at + CHUNK).min(to);
        let hay_start = at.saturating_sub(BEHIND);
        let hay_end = (chunk_end + OVERLAP).min(len);
        let hay = snap.read(hay_start..hay_end)?;
        let base = hay_start;
        let span = (at - base) as usize..hay.len();
        let limit_rel = (chunk_end - base) as usize;
        let (found, cut) = m.find_in(&hay, span, limit_rel, hay_end < len);
        let mut next = chunk_end;
        for r in found {
            out.push((base + r.start as u64, base + r.end as u64));
            next = next.max(base + r.end as u64);
            if out.len() >= limit {
                return Ok(out);
            }
        }
        if let Some(s) = cut {
            let start = base + s as u64;
            let end = long_match(snap, m, start)?;
            out.push((start, end));
            next = next.max(end);
        }
        at = next;
    }
    Ok(out)
}

/// The full match starting at `start`, read with up to [`MAX_MATCH`] bytes.
fn long_match(snap: &Snapshot, m: &Matcher, start: u64) -> io::Result<u64> {
    let len = snap.len();
    let hay_start = start.saturating_sub(BEHIND);
    let hay_end = (start + MAX_MATCH).min(len);
    let hay = snap.read(hay_start..hay_end)?;
    let input = Input::new(&hay[..])
        .span((start - hay_start) as usize..hay.len())
        .anchored(Anchored::Yes);
    Ok(m.re
        .search(&input)
        .map_or(start + 1, |x| hay_start + x.end() as u64))
}

/// Receives batches of matches, in order, with progress.
pub type OnBatch<'a> = dyn FnMut(&[(u64, u64)], &SearchProgress) + 'a;
/// Receives each match and its replacement text.
pub type OnReplacement<'a> = dyn FnMut((u64, u64), &[u8]) -> io::Result<ControlFlow<()>> + 'a;

/// Progress of a full search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub matches: u64,
    /// More matches exist than were stored.
    pub truncated: bool,
    pub complete: bool,
}

/// Searches the whole snapshot in parallel chunks on `scheduler`, calling
/// `on_batch(matches, progress)` in document order as chunks finish.
/// Returns early (incomplete) when `cancel` fires.
pub fn search_all(
    snap: &Snapshot,
    m: &Matcher,
    scheduler: &Scheduler,
    parallel: usize,
    cancel: &CancelToken,
    on_batch: &mut OnBatch<'_>,
) -> io::Result<SearchProgress> {
    let len = snap.len();
    let chunks: Vec<(u64, u64)> = (0..len.div_ceil(CHUNK))
        .map(|i| (i * CHUNK, ((i + 1) * CHUNK).min(len)))
        .collect();
    let mut progress = SearchProgress {
        bytes_total: len,
        ..SearchProgress::default()
    };
    let mut stored = 0usize;
    let mut prev_end = 0u64;
    let window = parallel.max(1) * 2;
    let mut handles = std::collections::VecDeque::new();
    let mut next = 0;
    while next < chunks.len() || !handles.is_empty() {
        while next < chunks.len() && handles.len() < window {
            let (a, b) = chunks[next];
            let (snap, m, c) = (snap.clone(), m.clone(), cancel.clone());
            handles.push_back((
                (a, b),
                scheduler.submit(
                    JobSpec::new("txt-search", Priority::NearViewport),
                    move |ctx| {
                        if c.is_cancelled() || ctx.is_cancelled() {
                            return Ok(Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "cancelled",
                            )));
                        }
                        Ok(search_range(&snap, &m, a, b, usize::MAX))
                    },
                ),
            ));
            next += 1;
        }
        let ((a, b), h) = handles.pop_front().expect("non-empty");
        if cancel.is_cancelled() {
            for (_, h) in handles {
                h.cancel();
            }
            return Ok(progress);
        }
        let result = h.wait().map_err(|e| io::Error::other(e.to_string()))?;
        let mut found = match Arc::try_unwrap(result) {
            Ok(r) => r?,
            Err(shared) => match &*shared {
                Ok(v) => v.clone(),
                Err(e) => return Err(io::Error::new(e.kind(), e.to_string())),
            },
        };
        // Keep leftmost-first semantics across the chunk seam.
        if found.first().is_some_and(|&(s, _)| s < prev_end) {
            found = search_range(snap, m, prev_end.max(a), b, usize::MAX)?;
        }
        found.retain(|&(s, _)| s >= prev_end);
        if let Some(&(_, e)) = found.last() {
            prev_end = e;
        }
        progress.matches += found.len() as u64;
        progress.bytes_done = b;
        let keep = (MAX_STORED - stored).min(found.len());
        stored += keep;
        if keep < found.len() {
            progress.truncated = true;
        }
        on_batch(&found[..keep], &progress);
    }
    progress.complete = true;
    on_batch(&[], &progress);
    Ok(progress)
}

/// Next match at or after `from`, wrapping to the start; scans in chunks.
pub fn find_next(snap: &Snapshot, m: &Matcher, from: u64) -> io::Result<Option<(u64, u64)>> {
    if let Some(&hit) = search_range(snap, m, from, snap.len(), 1)?.first() {
        return Ok(Some(hit));
    }
    Ok(search_range(snap, m, 0, from, 1)?.first().copied())
}

/// Last match ending at or before `before`, wrapping to the end.
pub fn find_prev(snap: &Snapshot, m: &Matcher, before: u64) -> io::Result<Option<(u64, u64)>> {
    let back = |lo: u64, hi: u64| -> io::Result<Option<(u64, u64)>> {
        let mut end = hi;
        while end > lo {
            let start = end.saturating_sub(CHUNK).max(lo);
            let hits = search_range(snap, m, start, end, usize::MAX)?;
            if let Some(&h) = hits.iter().rev().find(|h| h.1 <= hi) {
                return Ok(Some(h));
            }
            end = start;
        }
        Ok(None)
    };
    if let Some(h) = back(0, before)? {
        return Ok(Some(h));
    }
    back(before, snap.len())
}

/// The replacement text for the match `at` (regex: `$1`, `${name}`
/// expanded; literal: as is).
pub fn expand(
    snap: &Snapshot,
    m: &Matcher,
    at: (u64, u64),
    replacement: &str,
) -> io::Result<Vec<u8>> {
    if !m.query.regex {
        return Ok(replacement.as_bytes().to_vec());
    }
    let hay_start = at.0.saturating_sub(BEHIND);
    let hay_end = (at.1 + OVERLAP).min(snap.len());
    let hay = snap.read(hay_start..hay_end)?;
    let mut caps = m.re.create_captures();
    let input = Input::new(&hay[..])
        .span((at.0 - hay_start) as usize..hay.len())
        .anchored(Anchored::Yes);
    m.re.search_captures(&input, &mut caps);
    let mut out = Vec::new();
    caps.interpolate_bytes_into(&hay, replacement.as_bytes(), &mut out);
    Ok(out)
}

/// Calls `f` with each match and its expansion, in order (replace-all).
pub fn for_each_replacement(
    snap: &Snapshot,
    m: &Matcher,
    replacement: &str,
    cancel: &CancelToken,
    f: &mut OnReplacement<'_>,
) -> io::Result<bool> {
    let len = snap.len();
    let mut at = 0;
    let mut caps = m.re.create_captures();
    let mut out = Vec::new();
    while at < len {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let hits = search_range(snap, m, at, (at + CHUNK).min(len), usize::MAX)?;
        let mut next = (at + CHUNK).min(len);
        if m.query.regex && !hits.is_empty() {
            let lo = hits[0].0.saturating_sub(BEHIND);
            let hi = (hits.last().expect("non-empty").1 + OVERLAP).min(len);
            let hay = snap.read(lo..hi)?;
            for &h in &hits {
                let input = Input::new(&hay[..])
                    .span((h.0 - lo) as usize..hay.len())
                    .anchored(Anchored::Yes);
                m.re.search_captures(&input, &mut caps);
                out.clear();
                caps.interpolate_bytes_into(&hay, replacement.as_bytes(), &mut out);
                if f(h, &out)?.is_break() {
                    return Ok(false);
                }
                next = next.max(h.1);
            }
        } else {
            for &h in &hits {
                if f(h, replacement.as_bytes())?.is_break() {
                    return Ok(false);
                }
                next = next.max(h.1);
            }
        }
        at = next;
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
