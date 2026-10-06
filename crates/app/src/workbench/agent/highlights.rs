//! Syntax colors for the code blocks Kit's `TextView` finds in replies. Kit asks while it
//! paints, so the answer is a table lookup; a miss goes to a task that highlights it in the
//! background and swaps in a new table (and a new highlighter, so Kit drops its cached colors).

use super::*;
use gpui_kit::base::text::CodeBlock;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Mutex,
};

pub(in crate::workbench) type Highlighter = Arc<dyn Fn(&CodeBlock) -> markdown::Runs + Send + Sync>;

/// Blocks kept highlighted; past this the table starts over (what is on screen comes back).
const MAX_BLOCKS: usize = 256;

/// (key, fence tag, code)
type Miss = (u64, SharedString, SharedString);

pub(in crate::workbench) struct CodeHighlights {
    done: Arc<HashMap<u64, markdown::Runs>>,
    /// Sent and not answered yet: a block is painted on every frame until then.
    asked: Arc<Mutex<HashSet<u64>>>,
    misses: async_channel::Sender<Miss>,
    /// Bumped when the colors change; answers worked out before that are dropped.
    generation: u64,
    pub highlighter: Highlighter,
    _task: Task<()>,
}

fn block_key(tag: &str, code: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    (tag, code).hash(&mut hasher);
    hasher.finish()
}

fn highlighter(
    done: Arc<HashMap<u64, markdown::Runs>>,
    asked: Arc<Mutex<HashSet<u64>>>,
    misses: async_channel::Sender<Miss>,
) -> Highlighter {
    Arc::new(move |block: &CodeBlock| {
        let tag = block.lang().unwrap_or_default();
        if markdown::fence_language(&tag).is_none() {
            return Vec::new();
        }
        let code = block.code();
        let key = block_key(&tag, &code);
        if let Some(runs) = done.get(&key) {
            return runs.clone();
        }
        let mut asked = asked.lock().unwrap();
        if asked.insert(key) && misses.try_send((key, tag, code)).is_err() {
            asked.remove(&key);
        }
        Vec::new()
    })
}

impl CodeHighlights {
    pub(in crate::workbench) fn new(cx: &mut Context<Workbench>) -> Self {
        let (misses, receiver) = async_channel::bounded::<Miss>(64);
        // Waits on the channel: no work while nothing new is painted.
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                let Ok((theme, generation)) = this.update(cx, |this, cx| {
                    let theme = gpui_kit::component::Theme::global(cx)
                        .highlight_theme
                        .clone();
                    (theme, this.agent.code.generation)
                }) else {
                    break;
                };
                let runs = cx
                    .background_spawn(async move {
                        batch
                            .into_iter()
                            .map(|(key, tag, code)| (key, markdown::highlight(&tag, &code, &theme)))
                            .collect::<Vec<_>>()
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.agent.code.add(generation, runs) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let done = Arc::new(HashMap::new());
        let asked = Arc::new(Mutex::new(HashSet::new()));
        Self {
            highlighter: highlighter(done.clone(), asked.clone(), misses.clone()),
            done,
            asked,
            misses,
            generation: 0,
            _task: task,
        }
    }

    /// Takes in highlighted blocks; `false` when they were worked out in old colors.
    fn add(&mut self, generation: u64, runs: Vec<(u64, markdown::Runs)>) -> bool {
        if generation != self.generation {
            return false;
        }
        let mut done = if self.done.len() + runs.len() > MAX_BLOCKS {
            HashMap::new()
        } else {
            (*self.done).clone()
        };
        {
            let mut asked = self.asked.lock().unwrap();
            for (key, runs) in runs {
                asked.remove(&key);
                done.insert(key, runs);
            }
        }
        self.done = Arc::new(done);
        self.highlighter = highlighter(self.done.clone(), self.asked.clone(), self.misses.clone());
        true
    }

    #[cfg(test)]
    pub(in crate::workbench) fn highlighted(&self) -> usize {
        self.done.values().filter(|runs| !runs.is_empty()).count()
    }

    /// The colors changed: every block is highlighted again when it is next painted.
    pub(in crate::workbench) fn clear(&mut self) {
        self.generation += 1;
        self.asked.lock().unwrap().clear();
        self.done = Arc::new(HashMap::new());
        self.highlighter = highlighter(self.done.clone(), self.asked.clone(), self.misses.clone());
    }
}
