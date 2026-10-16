//! Splits a streamed answer into "thinking" and visible content.
//!
//! Some servers (llama-server without a reasoning parser, older Ollama builds)
//! leave Qwen's `<think>...</think>` blocks inline in the content stream. Tags can
//! arrive split across chunks, so the splitter holds back any trailing text that
//! could be the start of a tag until the next chunk resolves it.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Reasoning(String),
    Content(String),
    /// A `</think>` arrived without an opening tag (the chat template already
    /// opened it), so all content received so far was actually reasoning.
    Reclassify,
}

#[derive(Debug, Default)]
pub struct ThinkSplitter {
    buf: String,
    in_think: bool,
    seen_block: bool,
}

impl ThinkSplitter {
    pub fn push(&mut self, text: &str) -> Vec<Piece> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        loop {
            if self.in_think {
                if let Some(i) = self.buf.find(CLOSE) {
                    if i > 0 {
                        out.push(Piece::Reasoning(self.buf[..i].to_string()));
                    }
                    self.buf.drain(..i + CLOSE.len());
                    self.in_think = false;
                    self.seen_block = true;
                    continue;
                }
                self.emit_safe(&[CLOSE], &mut out);
                break;
            }

            let open = self.buf.find(OPEN);
            let close = self.buf.find(CLOSE);
            match (open, close) {
                (Some(o), c) if c.map_or(true, |c| o < c) => {
                    if o > 0 {
                        out.push(Piece::Content(self.buf[..o].to_string()));
                    }
                    self.buf.drain(..o + OPEN.len());
                    self.in_think = true;
                }
                (_, Some(c)) => {
                    if c > 0 {
                        out.push(Piece::Content(self.buf[..c].to_string()));
                    }
                    self.buf.drain(..c + CLOSE.len());
                    if !self.seen_block {
                        out.push(Piece::Reclassify);
                        self.seen_block = true;
                    }
                }
                _ => {
                    self.emit_safe(&[OPEN, CLOSE], &mut out);
                    break;
                }
            }
        }
        out
    }

    /// Flushes whatever is still held back once the stream has ended.
    pub fn finish(&mut self) -> Vec<Piece> {
        let rest = std::mem::take(&mut self.buf);
        if rest.is_empty() {
            Vec::new()
        } else if self.in_think {
            vec![Piece::Reasoning(rest)]
        } else {
            vec![Piece::Content(rest)]
        }
    }

    /// Emits everything except a trailing partial tag.
    fn emit_safe(&mut self, tags: &[&str], out: &mut Vec<Piece>) {
        let keep = partial_tag_suffix(&self.buf, tags);
        let end = self.buf.len() - keep;
        if end == 0 {
            return;
        }
        let text: String = self.buf.drain(..end).collect();
        out.push(if self.in_think {
            Piece::Reasoning(text)
        } else {
            Piece::Content(text)
        });
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of one of `tags`.
/// Tags are ASCII, so the cut point is always a char boundary.
fn partial_tag_suffix(s: &str, tags: &[&str]) -> usize {
    let mut best = 0;
    for tag in tags {
        for k in (1..tag.len()).rev() {
            if k > best && s.ends_with(&tag[..k]) {
                best = k;
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String, bool) {
        let mut s = ThinkSplitter::default();
        let (mut reasoning, mut content, mut reclassified) = (String::new(), String::new(), false);
        let mut pieces = Vec::new();
        for c in chunks {
            pieces.extend(s.push(c));
        }
        pieces.extend(s.finish());
        for p in pieces {
            match p {
                Piece::Reasoning(t) => reasoning.push_str(&t),
                Piece::Content(t) => content.push_str(&t),
                Piece::Reclassify => {
                    reasoning.push_str(&std::mem::take(&mut content));
                    reclassified = true;
                }
            }
        }
        (reasoning, content, reclassified)
    }

    #[test]
    fn plain_content_passes_through() {
        assert_eq!(run(&["Hello ", "world"]), ("".into(), "Hello world".into(), false));
    }

    #[test]
    fn think_block_in_one_chunk() {
        assert_eq!(run(&["<think>abc</think>Answer"]), ("abc".into(), "Answer".into(), false));
    }

    #[test]
    fn tags_split_across_chunks() {
        assert_eq!(
            run(&["<thi", "nk>ab", "c</th", "ink>Ans", "wer"]),
            ("abc".into(), "Answer".into(), false)
        );
    }

    #[test]
    fn stray_close_tag_reclassifies_prior_content() {
        assert_eq!(run(&["reasoning here", "</think>", "Answer"]), ("reasoning here".into(), "Answer".into(), true));
    }

    #[test]
    fn angle_brackets_that_are_not_tags_survive() {
        assert_eq!(run(&["a <", " b and <t", "able>"]), ("".into(), "a < b and <table>".into(), false));
    }

    #[test]
    fn unclosed_think_is_reasoning() {
        assert_eq!(run(&["<think>still thinking"]), ("still thinking".into(), "".into(), false));
    }

    #[test]
    fn multibyte_text_is_preserved() {
        assert_eq!(run(&["<think>思考", "</think>答案 ✓"]), ("思考".into(), "答案 ✓".into(), false));
    }
}
