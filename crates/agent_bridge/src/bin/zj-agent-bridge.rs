//! The bridge on its own, for development and `agent_client`'s real-agent smoke test
//! (`ZJ_AGENT_BRIDGE=target/release/zj-agent-bridge`). ZJ itself runs the bridge as
//! `workspace-editor --agent-bridge …`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match args.split_first() {
        Some((flag, rest)) if flag == workspace_editor_agent_bridge::FLAG => rest.to_vec(),
        _ => args,
    };
    std::process::exit(workspace_editor_agent_bridge::run(&args));
}
