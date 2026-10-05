//! ⇧⌘P 命令面板的命令表：每个面向用户的 action 一行，名称参照 VS Code 中文版。
//! 快捷键不写在表里：打开面板时按当时的焦点用 GPUI 的 keymap 查询
//! （`Workbench::open_quick_open_with`），显示用欢迎页的 keycap 样式。
//!
//! 没有收进来的 action：
//! - 资源管理器（NewFile、Rename、Delete、CopyFiles 等）：动作挂在 `Explorer` 键上下文的
//!   节点上，作用对象是树里选中的行；从面板分发时焦点不在资源管理器，全部是空操作。
//! - Diff 编辑器（CopyDiff、SelectAllDiff、AcceptAgentChange、RejectAgentChange）：处理函数
//!   挂在 `DiffEditor` 节点上，且复制依赖鼠标拖出的选区。
//! - 查找浮层内（ReplaceOne、ReplaceAll、ToggleFindCase 等 6 个）：只在 `FindWidget` 上下
//!   文里、浮层打开时有意义。
//! - 纯内部的（Kit 的 Enter / Escape / Tab、列表导航）。

use super::navigation::{FindReferences, GoToLine, GoToSymbol, NavigateBack, NavigateForward};
use super::*;
use gpui_kit::{
    Action, App, Keystroke, Modifiers, Window,
    component::{input::GoToDefinition, kbd::Kbd},
};

pub(super) struct Command {
    pub name: &'static str,
    pub action: fn() -> Box<dyn Action>,
}

macro_rules! command {
    ($name:literal, $action:expr) => {
        Command {
            name: $name,
            action: || Box::new($action),
        }
    };
}

gpui_kit::actions!(commands, [ShowAllCommands]);

pub(super) const COMMANDS: &[Command] = &[
    command!("文件: 新建文件", NewUntitled),
    command!("文件: 新建窗口", NewWindow),
    command!("文件: 打开文件…", OpenFile),
    command!("文件: 打开文件夹…", OpenFolder),
    command!("文件: 保存", Save),
    command!("文件: 另存为…", SaveAs),
    command!("文件: 全部保存", SaveAll),
    command!("文件: 自动保存: 关闭", AutoSaveOff),
    command!("文件: 自动保存: 编辑后 1 秒", AutoSaveAfterDelay),
    command!("文件: 自动保存: 失去焦点时", AutoSaveOnFocusChange),
    command!("文件: 更改行尾序列", ToggleLineEnding),
    command!("文件: 关闭编辑器", CloseEditor),
    command!("文件: 关闭其他编辑器", CloseOtherEditors),
    command!("文件: 关闭所有编辑器", CloseAllEditors),
    command!("视图: 重新打开已关闭的编辑器", ReopenClosedEditor),
    command!("文件: 复制活动文件的路径", CopyActivePath),
    command!("文件: 复制活动文件的相对路径", CopyActiveRelativePath),
    command!("文件: 在 Finder 中显示活动文件", RevealActiveInFinder),
    command!("文件: 在资源管理器视图中显示", RevealActiveInExplorer),
    command!("文件: 退出 ZJ", Quit),
    command!("视图: 转到文件…", QuickOpenFile),
    command!("视图: 显示所有命令", ShowAllCommands),
    command!("视图: 切换主侧栏", ToggleSidebar),
    command!("视图: 切换自动换行", ToggleSoftWrap),
    command!("视图: 放大", ZoomIn),
    command!("视图: 缩小", ZoomOut),
    command!("视图: 重置缩放", ZoomReset),
    command!("视图: 显示 / 隐藏点文件", ToggleHiddenFiles),
    command!("视图: 切换 Agent 面板", ToggleAgentPanel),
    command!("转到: 转到行/列…", GoToLine),
    command!("转到: 转到文件中的符号…", GoToSymbol),
    command!("转到: 转到定义", GoToDefinition),
    command!("转到: 查找所有引用", FindReferences),
    command!("转到: 返回", NavigateBack),
    command!("转到: 前进", NavigateForward),
    command!("编辑: 切换行注释", ToggleLineComment),
    command!("编辑: 向上移动行", MoveLinesUp),
    command!("编辑: 向下移动行", MoveLinesDown),
    command!("编辑: 向上复制行", CopyLinesUp),
    command!("编辑: 向下复制行", CopyLinesDown),
    command!("编辑: 选择下一个匹配项", SelectNextOccurrence),
    command!("查找: 查找", FindInFile),
    command!("查找: 替换", FindReplace),
    command!("查找: 查找下一个", FindNext),
    command!("查找: 查找上一个", FindPrevious),
    command!("查找: 在文件中查找", FindInFiles),
    command!("Markdown: 切换预览", ToggleMarkdownPreview),
    command!("终端: 新建终端", NewTerminal),
    command!("终端: 切换终端", ToggleTerminal),
    command!("终端: 拆分终端", SplitTerminal),
    command!("终端: 关闭终端", KillTerminal),
    command!("Agent: 新建会话", NewAgentSession),
    command!("Agent: 搜索会话…", SearchSessions),
    command!("Agent: 附加当前文件或选区", AddSelectionToAgent),
    command!("Agent: 下一个待审批", NextApproval),
];

/// 打开面板时按当时的焦点解析每个命令的最高优先级绑定；没有焦点时用当前上下文栈。
/// 返回与 [`COMMANDS`] 平行的 keycap 标签（多键组合拍平，欢迎页风格）。
pub(super) fn shortcut_keys(window: &Window, cx: &App) -> Vec<Vec<String>> {
    let focus = window.focused(cx);
    COMMANDS
        .iter()
        .map(|command| {
            let action = (command.action)();
            let binding = match &focus {
                Some(focus) => {
                    window.highest_precedence_binding_for_action_in(action.as_ref(), focus)
                }
                None => window.highest_precedence_binding_for_action(action.as_ref()),
            };
            binding
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .flat_map(|key| keycaps(key.as_keystroke()))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// 一个按键的 keycap 标签：⌃ ⌥ ⇧ ⌘，然后是键本身（与欢迎页的顺序一致）。
fn keycaps(stroke: &Keystroke) -> Vec<String> {
    let mut keys = Vec::new();
    if stroke.modifiers.control {
        keys.push("⌃".into());
    }
    if stroke.modifiers.alt {
        keys.push("⌥".into());
    }
    if stroke.modifiers.shift {
        keys.push("⇧".into());
    }
    if stroke.modifiers.platform {
        keys.push("⌘".into());
    }
    let bare = Keystroke {
        modifiers: Modifiers::default(),
        ..stroke.clone()
    };
    keys.push(Kbd::format(&bare));
    keys
}

#[cfg(test)]
mod tests {
    use gpui_kit::Keystroke;

    #[test]
    fn command_names_are_unique() {
        let mut names: Vec<&str> = super::COMMANDS.iter().map(|c| c.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "命令表里有重名");
        assert!(total > 40);
    }

    #[test]
    fn keystrokes_become_keycaps() {
        let stroke = Keystroke::parse("cmd-shift-p").unwrap();
        assert_eq!(super::keycaps(&stroke), ["⇧", "⌘", "P"]);
        let bare = Keystroke::parse("f12").unwrap();
        assert_eq!(super::keycaps(&bare), ["F12"]);
        let chord = Keystroke::parse("ctrl-`").unwrap();
        assert_eq!(super::keycaps(&chord), ["⌃", "`"]);
    }
}
