use rayon::prelude::*;
use regex::Regex;
use tokenizers::Tokenizer;

#[derive(Clone)]
pub struct BgeChunker {
    tokenizer: Tokenizer,
}

impl BgeChunker {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let tokenizer = Tokenizer::from_file(path)
            .map_err(|e| anyhow::anyhow!("Tokenizer error at {}: {}", path, e))?;
        Ok(Self { tokenizer })
    }

    pub fn split_sentences(&self, text: &str) -> Vec<String> {
        let text_clean = text.replace('\n', " ");
        let re = Regex::new(r"([.!?])\s+([A-ZÅÄÖ])").unwrap();
        let mut sentences = Vec::new();
        let mut start = 0;

        for cap in re.captures_iter(&text_clean) {
            let end = cap.get(1).unwrap().end();
            let sentence = text_clean[start..end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
            start = end;
        }

        if start < text_clean.len() {
            let sentence = text_clean[start..].trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
        }

        sentences
    }

    pub fn count_tokens(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().len())
            .unwrap_or(0)
    }

    pub fn chunk_text(&self, text: &str, max_tokens: usize, overlap_tokens: usize) -> Vec<String> {
        let sentences = self.split_sentences(text);
        if sentences.is_empty() {
            return Vec::new();
        }

        // Tokenization dominates cost; parallelize it for large inputs while keeping
        // chunking itself sequential so overlap across chunk boundaries stays correct.
        let token_counts: Vec<usize> = if sentences.len() > 500 {
            sentences.par_iter().map(|s| self.count_tokens(s)).collect()
        } else {
            sentences.iter().map(|s| self.count_tokens(s)).collect()
        };

        self.chunk_text_with_counts(&sentences, &token_counts, max_tokens, overlap_tokens)
    }

    fn chunk_text_with_counts(
        &self,
        sentences: &[String],
        token_counts: &[usize],
        max_tokens: usize,
        overlap_tokens: usize,
    ) -> Vec<String> {
        debug_assert_eq!(sentences.len(), token_counts.len());

        let mut chunks = Vec::new();
        let mut current: Vec<(String, usize)> = Vec::new();
        let mut current_tokens = 0usize;

        for (sent, &tokens) in sentences.iter().zip(token_counts.iter()) {
            if current_tokens + tokens > max_tokens && !current.is_empty() {
                chunks.push(join_sentences(&current));

                let mut overlap: Vec<(String, usize)> = Vec::new();
                let mut overlap_count = 0usize;
                for (s, sn) in current.iter().rev() {
                    if overlap_count + sn <= overlap_tokens {
                        overlap.insert(0, (s.clone(), *sn));
                        overlap_count += sn;
                    } else {
                        break;
                    }
                }
                current = overlap;
                current_tokens = overlap_count;
            }
            current.push((sent.clone(), tokens));
            current_tokens += tokens;
        }

        if !current.is_empty() {
            chunks.push(join_sentences(&current));
        }
        chunks
    }
}

fn join_sentences(items: &[(String, usize)]) -> String {
    items
        .iter()
        .map(|(s, _)| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn chunk_text_exact(
    text: &str,
    max_tokens: usize,
    overlap_tokens: usize,
    tokenizer_path: &str,
) -> anyhow::Result<Vec<String>> {
    let chunker = BgeChunker::new(tokenizer_path)?;
    Ok(chunker.chunk_text(text, max_tokens, overlap_tokens))
}
