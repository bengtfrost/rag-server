use regex::Regex;
use tokenizers::Tokenizer;

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

    fn count_tokens(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().len())
            .unwrap_or(0)
    }

    pub fn chunk_text(&self, text: &str, max_tokens: usize, overlap_tokens: usize) -> Vec<String> {
        let sentences = self.split_sentences(text);
        let mut chunks = Vec::new();
        let mut current_chunk: Vec<String> = Vec::new();
        let mut current_tokens = 0;

        for sent in sentences {
            let tokens = self.count_tokens(&sent);
            if current_tokens + tokens > max_tokens && !current_chunk.is_empty() {
                chunks.push(current_chunk.join(" "));

                let mut overlap_vec = Vec::new();
                let mut overlap_count = 0;
                for s in current_chunk.iter().rev() {
                    let sn = self.count_tokens(s);
                    if overlap_count + sn <= overlap_tokens {
                        overlap_vec.insert(0, s.clone());
                        overlap_count += sn;
                    } else {
                        break;
                    }
                }
                current_chunk = overlap_vec;
                current_tokens = overlap_count;
            }
            current_chunk.push(sent);
            current_tokens += tokens;
        }
        if !current_chunk.is_empty() {
            chunks.push(current_chunk.join(" "));
        }
        chunks
    }
}

// Extern wrapper för CLI-användning
pub fn chunk_text_exact(
    text: &str,
    max_tokens: usize,
    overlap_tokens: usize,
    tokenizer_path: &str,
) -> anyhow::Result<Vec<String>> {
    let chunker = BgeChunker::new(tokenizer_path)?;
    Ok(chunker.chunk_text(text, max_tokens, overlap_tokens))
}
