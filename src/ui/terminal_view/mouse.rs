//! mouse buttons and motion: selection, reporting to the program and clicks on links

use super::*;

impl TerminalView {
    /// tell the program about the mouse when it asked for it, true when it did
    pub(super) fn report_mouse(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        action: MouseAction,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let terminal = self.terminal.read(cx);
        if !terminal.owns_mouse(modifiers) {
            return false;
        }
        let cell = terminal.mouse_cell(position);
        // programs only care about the cell, not every pixel of motion inside it
        if action == MouseAction::Motion && self.last_mouse_cell == Some(cell) {
            return true;
        }
        self.last_mouse_cell = Some(cell);
        self.terminal.update(cx, |term, _| {
            term.report_mouse(cell, button, action, modifiers)
        });
        true
    }

    /// a press goes to the program when it asked for the mouse, otherwise a left click starts a
    /// selection: single click, double selects words, triple lines, shift extends. a right click
    /// copies the selection
    pub fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.hints.take().is_some() {
            cx.notify();
        }
        // a click with the link modifier opens the link, even when the program owns the mouse
        if event.button == gpui::MouseButton::Left
            && let Some(link) = self.link_at(event.position, &event.modifiers, cx)
        {
            self.run_link(&link, cx);
            return;
        }
        if let Some(button) = MouseButton::from_gpui(event.button)
            && self.report_mouse(
                event.position,
                button,
                MouseAction::Press,
                &event.modifiers,
                cx,
            )
        {
            return;
        }
        if event.button == gpui::MouseButton::Right && Settings::get(cx).terminal.right_click_copy {
            self.copy_selection(cx);
            return;
        }
        if event.button != gpui::MouseButton::Left {
            return;
        }
        let ty = match event.click_count {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let hold = Settings::get(cx).selection.hold_frame;
        self.terminal.update(cx, |term, _| {
            if event.modifiers.shift && ty == SelectionType::Simple {
                term.extend_selection(event.position);
            } else {
                if hold {
                    term.freeze();
                }
                term.start_selection(event.position, ty);
            }
        });
        self.selecting = true;
        cx.notify();
    }

    /// extend the selection while the button is held, even outside the terminal area,
    /// or report the motion to a program that asked for it
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.last_mouse = Some((event.position, event.modifiers));
        if event.pressed_button.is_none() {
            self.update_hovered_link(cx);
            self.update_peek(event.position, cx);
        }
        if self.selecting {
            self.terminal
                .update(cx, |term, _| term.extend_selection(event.position));
            cx.notify();
            return;
        }
        if self.dragging_scrollbar {
            return;
        }
        let button = match event.pressed_button {
            Some(button) => match MouseButton::from_gpui(button) {
                Some(button) => button,
                None => return,
            },
            None => MouseButton::None,
        };
        self.report_mouse(
            event.position,
            button,
            MouseAction::Motion,
            &event.modifiers,
            cx,
        );
    }

    /// finish the drag, the selection stays until the next click or input
    pub fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting && Settings::get(cx).terminal.select_to_copy {
            self.copy_selection(cx);
        }
        if self.selecting {
            self.terminal.update(cx, |term, _| term.unfreeze());
        }
        let click = self.selecting
            && event.button == gpui::MouseButton::Left
            && event.click_count == 1
            && !event.modifiers.modified()
            && Settings::get(cx).shell_integration.click_moves_cursor;
        if click && self.terminal.read(cx).selection_text().is_none() {
            let moved = self
                .terminal
                .update(cx, |term, _| term.click_moves_cursor(event.position));
            if moved {
                self.input_happened(cx);
            }
        }
        let was_local = self.selecting || self.dragging_scrollbar;
        self.selecting = false;
        self.dragging_scrollbar = false;
        if !was_local && let Some(button) = MouseButton::from_gpui(event.button) {
            self.report_mouse(
                event.position,
                button,
                MouseAction::Release,
                &event.modifiers,
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::ui::terminal_view::tests::*;

    /// select the whole screen once the shell echoed "picked", and put "before" in the clipboard
    fn select_echoed(view: &Entity<TerminalView>, cx: &mut VisualTestContext) {
        view.update(cx, |view, cx| {
            view.terminal
                .update(cx, |term, _| term.type_text(b"echo picked\r"))
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !view.update(cx, |view, cx| {
            view.terminal.update(cx, |term, _| {
                term.sync();
                term.select_all();
                term.selection_text()
                    .is_some_and(|text| text.contains("picked"))
            })
        }) {
            assert!(Instant::now() < deadline, "shell never echoed");
            std::thread::sleep(Duration::from_millis(50));
        }
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    }

    /// release the left button after a drag over the whole screen, returns the clipboard text
    fn drag_and_release(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> Option<String> {
        select_echoed(view, cx);
        view.update(cx, |view, cx| {
            view.selecting = true;
            view.mouse_up(
                &MouseUpEvent {
                    button: gpui::MouseButton::Left,
                    ..MouseUpEvent::default()
                },
                cx,
            );
        });
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn select_to_copy_copies_on_release(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"select_to_copy": true}}"#, cx);
        let copied = drag_and_release(&view, cx);
        assert!(copied.is_some_and(|text| text.contains("picked")));
    }

    #[gpui::test]
    fn release_does_not_copy_without_select_to_copy(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"select_to_copy": false}}"#, cx);
        assert_eq!(drag_and_release(&view, cx).as_deref(), Some("before"));
    }

    /// select the whole screen once the shell echoed, then right click, returns the clipboard text
    fn select_and_right_click(
        view: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
    ) -> Option<String> {
        select_echoed(view, cx);
        view.update(cx, |view, cx| {
            view.mouse_down(
                &MouseDownEvent {
                    button: gpui::MouseButton::Right,
                    ..MouseDownEvent::default()
                },
                cx,
            )
        });
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn right_click_copies_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"right_click_copy": true}}"#, cx);
        let copied = select_and_right_click(&view, cx);
        assert!(copied.is_some_and(|text| text.contains("picked")));
    }

    #[gpui::test]
    fn right_click_does_not_copy_without_right_click_copy(cx: &mut TestAppContext) {
        let (view, cx) = focused_view(r#"{"terminal": {"right_click_copy": false}}"#, cx);
        assert_eq!(select_and_right_click(&view, cx).as_deref(), Some("before"));
    }

    #[gpui::test]
    fn a_click_ends_hints_mode(cx: &mut TestAppContext) {
        let (view, cx) = view_with_hints(cx);
        view.update(cx, |view, cx| {
            view.mouse_down(
                &MouseDownEvent {
                    button: gpui::MouseButton::Left,
                    ..MouseDownEvent::default()
                },
                cx,
            )
        });
        assert!(view.read_with(cx, |view, _| view.hints().is_none()));
        assert!(view.read_with(cx, |view, _| view.picked_hint().is_none()));
    }

    #[gpui::test]
    fn motion_reports_once_per_cell_and_skips_local_drags(cx: &mut TestAppContext) {
        let (view, cx) = focused_view("{}", cx);
        // any motion, in sgr form
        let file = raw_reader(
            &view,
            cx,
            "motion",
            "printf '\\033[?1003h\\033[?1006h'; ",
            |mode| mode.contains(TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE),
        );
        let at = |column: f32, line: f32, cx: &mut VisualTestContext| {
            view.read_with(cx, |view, cx| {
                let b = view.terminal.read(cx).last_content.terminal_bounds;
                point(
                    b.bounds.origin.x + b.cell_width * column,
                    b.bounds.origin.y + b.line_height * line,
                )
            })
        };
        let moved = |position, pressed_button, cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| {
                view.mouse_move(
                    &MouseMoveEvent {
                        position,
                        pressed_button,
                        modifiers: Modifiers::default(),
                    },
                    cx,
                )
            })
        };
        // two moves inside column 3 of line 2 tell it once
        moved(at(2.2, 1.5, cx), None, cx);
        moved(at(2.8, 1.5, cx), None, cx);
        // a button programs have no code for tells nothing
        let back = gpui::MouseButton::Navigate(gpui::NavigationDirection::Back);
        moved(at(4.5, 1.5, cx), Some(back), cx);
        // nor does dragging the scrollbar
        view.update(cx, |view, _| view.dragging_scrollbar = true);
        moved(at(6.5, 1.5, cx), Some(gpui::MouseButton::Left), cx);
        view.update(cx, |view, _| view.dragging_scrollbar = false);
        moved(at(8.5, 2.5, cx), None, cx);
        assert_eq!(raw_read(&view, cx, &file), "\x1b[<35;3;2M\x1b[<35;9;3M");
    }
}
