//! what plugins show in a window: the question before one first runs, its notices, its
//! panels and the marks it puts next to commands

use gpui::{App, Context, EntityId, Window};

use super::{Workspace, notifications::NotificationButton};
use crate::{
    settings::{Profile, Settings, Shell},
    terminal::GutterMark,
    theme::Theme,
};

impl Workspace {
    /// ask whether a plugin may run, `allow` runs once the user says yes
    pub fn ask_plugin_trust(
        &mut self,
        text: String,
        command: String,
        allow: impl Fn(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let buttons = vec![
            NotificationButton::new("allow", move |_, _, cx| allow(cx)),
            NotificationButton::new("deny", |_, _, _| {}),
        ];
        self.ask(text, Some(command), None, buttons, cx);
    }

    /// a notice a plugin sent
    pub fn show_plugin_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.show_notification(text, None, cx);
    }

    /// open a tab running `program` with `args`, a plugin's panel
    pub fn open_panel(
        &mut self,
        program: String,
        args: Vec<String>,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = Profile {
            command: Shell::WithArguments { program, args },
            ..Settings::get(cx).default_profile().clone()
        };
        let pane = self.focused;
        let before = self.pane(pane).map_or(0, |pane| pane.tabs.len());
        self.add_profile_tab(pane, &profile, true, window, cx);
        if let Some(name) = name
            && let Some(tab) = self
                .pane_mut(pane)
                .and_then(|pane| pane.tabs.get_mut(before))
        {
            tab.custom_title = Some(name);
        }
    }

    /// put a mark a plugin asked for on block `block` of tab `id`, the last one without it
    pub fn decorate_tab(
        &mut self,
        id: EntityId,
        block: Option<u64>,
        icon: String,
        tooltip: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (pane, ix) = self.find_id(id).ok_or("the pane is gone")?;
        let terminal = self.terminal_at(pane, ix, cx);
        let color = Theme::get(cx).text;
        terminal.update(cx, |terminal, _| {
            let block = match block {
                Some(block) if terminal.has_block(block) => block,
                Some(_) => return Err("no such block in the pane".to_string()),
                None => terminal
                    .blocks()
                    .back()
                    .map(|block| block.id)
                    .ok_or("the pane ran no command yet")?,
            };
            let mark = GutterMark {
                icon,
                color,
                tooltip,
            };
            terminal.decorate(|decorations| decorations.gutter.insert(block, mark));
            Ok(())
        })?;
        cx.notify();
        Ok(())
    }
}

/// build the titles of every window again, a plugin changed a block
pub fn refresh_all_titles(cx: &mut App) {
    for handle in cx.windows() {
        if let Some(workspace) = handle.downcast::<Workspace>() {
            workspace
                .update(cx, |workspace, _, _| workspace.refresh_titles())
                .ok();
        }
    }
}
