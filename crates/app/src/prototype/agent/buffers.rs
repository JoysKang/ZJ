//! The bridge that answers agents' `fs/read_text_file` with unsaved buffers from any window.

use super::*;

type BufferRequest = (PathBuf, async_channel::Sender<Option<String>>);

/// Answers the agents' `fs/read_text_file` with unsaved buffers from any window. The agent
/// threads only send requests; the UI thread reads the buffers when it gets to them.
struct BufferBridge(async_channel::Sender<BufferRequest>);

impl workspace_editor_agent::BufferProvider for BufferBridge {
    fn buffer_text(
        &self,
        path: &std::path::Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
        let (reply, answer) = async_channel::bounded(1);
        let sent = self.0.try_send((path.to_path_buf(), reply)).is_ok();
        Box::pin(async move {
            if sent {
                answer.recv().await.ok().flatten()
            } else {
                None
            }
        })
    }
}

struct AgentBuffers {
    bridge: Arc<BufferBridge>,
    _task: Task<()>,
}
impl Global for AgentBuffers {}

/// One bridge for the whole app, started with the first agent.
pub(super) fn buffer_provider(cx: &mut App) -> Arc<dyn workspace_editor_agent::BufferProvider> {
    if let Some(buffers) = cx.try_global::<AgentBuffers>() {
        return buffers.bridge.clone();
    }
    let (requests, incoming) = async_channel::unbounded::<BufferRequest>();
    let task = cx.spawn(async move |cx| {
        while let Ok((path, reply)) = incoming.recv().await {
            let text = cx.update(|cx| super::super::documents::buffer_text(&path, cx));
            let _ = reply.try_send(text);
        }
    });
    let bridge = Arc::new(BufferBridge(requests));
    cx.set_global(AgentBuffers {
        bridge: bridge.clone(),
        _task: task,
    });
    bridge
}
