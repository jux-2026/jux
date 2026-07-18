//! Terminal input handling for the TUI.
//!
//! Crossterm is the input parser; Ratatui only renders the application state.  Keeping
//! terminal reads on a dedicated thread prevents a burst of mouse reports from blocking
//! application updates (in particular `Ctrl-C` and `Esc`).  Consumers receive already
//! decoded [`crossterm::event::Event`] values and must not parse ANSI sequences themselves.

use crossterm::event::{self, Event};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Events emitted by [`EventHandler`].
#[derive(Debug)]
pub enum TuiEvent {
    /// A Crossterm event (key, mouse, resize, paste, ...).
    Terminal { event: Event, queued_at: Instant },
    /// A burst of wheel reports reduced to one scroll delta (five rows per report).
    ScrollDelta { delta: i32, column: u16, row: u16 },
    /// The reader stopped, usually because the application is shutting down.
    Closed,
}

/// Dedicated Crossterm event reader backed by a channel.
pub struct EventHandler {
    receiver: Receiver<TuiEvent>,
    pending: Option<TuiEvent>,
    reader: Option<JoinHandle<()>>,
}

impl EventHandler {
    /// Starts reading Crossterm events in a dedicated thread.
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut pending_scroll: Option<(i32, u16, u16)> = None;
            loop {
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            if let Some((delta, column, row)) = wheel_delta(&event) {
                                let entry = pending_scroll.get_or_insert((0, column, row));
                                entry.0 = entry.0.saturating_add(delta);
                                while event::poll(Duration::ZERO).unwrap_or(false) {
                                    let Ok(next) = event::read() else { break };
                                    if let Some((delta, _, _)) = wheel_delta(&next) {
                                        entry.0 = entry.0.saturating_add(delta);
                                    } else {
                                        if sender
                                            .send(TuiEvent::ScrollDelta {
                                                delta: entry.0,
                                                column: entry.1,
                                                row: entry.2,
                                            })
                                            .is_err()
                                        {
                                            return;
                                        }
                                        pending_scroll = None;
                                        if sender
                                            .send(TuiEvent::Terminal {
                                                event: next,
                                                queued_at: Instant::now(),
                                            })
                                            .is_err()
                                        {
                                            return;
                                        }
                                        break;
                                    }
                                }
                                if pending_scroll.is_some()
                                    && !event::poll(Duration::ZERO).unwrap_or(false)
                                {
                                    let (delta, column, row) =
                                        pending_scroll.take().expect("pending scroll");
                                    if sender
                                        .send(TuiEvent::ScrollDelta { delta, column, row })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            } else if sender
                                .send(TuiEvent::Terminal {
                                    event,
                                    queued_at: Instant::now(),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
            let _ = sender.send(TuiEvent::Closed);
        });
        Self {
            receiver,
            pending: None,
            reader: Some(reader),
        }
    }

    /// Waits for the next event.
    pub fn recv(&mut self) -> Result<TuiEvent, mpsc::RecvError> {
        let event = match self.pending.take() {
            Some(event) => event,
            None => self.receiver.recv()?,
        };
        Ok(coalesce_drag_events(
            event,
            &self.receiver,
            &mut self.pending,
        ))
    }

    /// Waits for the next event until the application must process background work.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<TuiEvent, RecvTimeoutError> {
        let event = match self.pending.take() {
            Some(event) => event,
            None => self.receiver.recv_timeout(timeout)?,
        };
        Ok(coalesce_drag_events(
            event,
            &self.receiver,
            &mut self.pending,
        ))
    }

    /// Returns the next event without blocking.
    pub fn try_recv(&mut self) -> Result<TuiEvent, TryRecvError> {
        let event = match self.pending.take() {
            Some(event) => event,
            None => self.receiver.try_recv()?,
        };
        Ok(coalesce_drag_events(
            event,
            &self.receiver,
            &mut self.pending,
        ))
    }
}

fn wheel_delta(event: &Event) -> Option<(i32, u16, u16)> {
    let Event::Mouse(mouse) = event else {
        return None;
    };
    let delta = match mouse.kind {
        crossterm::event::MouseEventKind::ScrollUp => 5,
        crossterm::event::MouseEventKind::ScrollDown => -5,
        _ => return None,
    };
    Some((delta, mouse.column, mouse.row))
}

fn coalesce_drag_events(
    first: TuiEvent,
    receiver: &Receiver<TuiEvent>,
    pending: &mut Option<TuiEvent>,
) -> TuiEvent {
    if !is_left_drag(&first) {
        return first;
    }
    let mut latest = first;
    while let Ok(next) = receiver.try_recv() {
        if is_left_drag(&next) {
            latest = next;
        } else {
            *pending = Some(next);
            break;
        }
    }
    latest
}

fn is_left_drag(event: &TuiEvent) -> bool {
    matches!(
        event,
        TuiEvent::Terminal {
            event: Event::Mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
                ..
            }),
            ..
        }
    )
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        // The reader is intentionally detached while Crossterm is in raw mode.  Once the
        // receiver is dropped, the next poll/read cycle exits cleanly; joining here could
        // otherwise wait for the poll timeout during terminal teardown.
        let _ = self.reader.take();
    }
}

#[cfg(test)]
mod tests {
    use super::{EventHandler, TuiEvent};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn queued_left_drags_keep_only_the_latest_position_and_preserve_mouse_up() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(mouse_event(MouseEventKind::Drag(MouseButton::Left), 10, 5))
            .expect("first drag is queued");
        sender
            .send(mouse_event(MouseEventKind::Drag(MouseButton::Left), 20, 10))
            .expect("second drag is queued");
        sender
            .send(mouse_event(MouseEventKind::Up(MouseButton::Left), 20, 10))
            .expect("mouse up is queued");
        let mut handler = EventHandler {
            receiver,
            pending: None,
            reader: None,
        };

        let latest = handler
            .recv_timeout(Duration::ZERO)
            .expect("latest drag is received");

        assert!(matches!(
            latest,
            TuiEvent::Terminal {
                event: crossterm::event::Event::Mouse(MouseEvent {
                    column: 20,
                    row: 10,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            handler
                .recv_timeout(Duration::ZERO)
                .expect("mouse up is preserved"),
            TuiEvent::Terminal {
                event: crossterm::event::Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    ..
                }),
                ..
            }
        ));
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> TuiEvent {
        TuiEvent::Terminal {
            event: crossterm::event::Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }),
            queued_at: Instant::now(),
        }
    }
}
