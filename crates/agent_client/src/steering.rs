//! The adapters' `_session/steering` extension, and ownership of a continuation's end.
//! Claude opts into `promptRequired` when idle. Codex can start a detached turn, whose end
//! is reported by `session_info_update._meta.codex.threadStatus` rather than a prompt reply.

use super::*;

#[derive(Default)]
pub(super) struct Steering {
    pub pending: Option<TurnId>,
    pub cancelled: bool,
    detached: bool,
    active_seen: bool,
    idle_seen: bool,
    deferred: Option<TurnOutcome>,
    pub followup: Option<Vec<PromptPart>>,
}

impl Steering {
    pub fn retire(&mut self) {
        let pending = self.pending.filter(|_| self.cancelled);
        *self = Self {
            pending,
            cancelled: pending.is_some(),
            ..Default::default()
        };
    }

    pub fn cancel_followup(&mut self) -> bool {
        self.cancelled = true;
        let waiting = self.followup.is_some() && self.deferred.is_some();
        self.followup = None;
        waiting
    }

    pub fn hold_end(&mut self, outcome: TurnOutcome) -> bool {
        if self.pending.is_some() || self.detached || self.followup.is_some() {
            self.deferred = Some(outcome);
            true
        } else {
            false
        }
    }

    pub fn continuation(&mut self) -> Option<Vec<PromptPart>> {
        if self.cancelled
            || self.pending.is_some()
            || self.deferred.is_none()
            || self.followup.is_none()
        {
            return None;
        }
        self.deferred = None;
        self.detached = false;
        self.active_seen = false;
        self.idle_seen = false;
        self.followup.take()
    }

    fn end_after_request(&mut self) -> Option<(TurnOutcome, bool)> {
        if self.detached && self.active_seen && self.idle_seen {
            Some((
                if self.cancelled {
                    TurnOutcome::Cancelled
                } else {
                    TurnOutcome::EndTurn
                },
                true,
            ))
        } else if !self.detached {
            self.deferred.take().map(|outcome| {
                (
                    if self.cancelled {
                        TurnOutcome::Cancelled
                    } else {
                        outcome
                    },
                    false,
                )
            })
        } else {
            None
        }
    }
}

pub(super) fn start(
    shared: &Arc<SessionState>,
    cx: &ConnectionTo<Agent>,
    session: &acp::SessionId,
    turn: TurnId,
    parts: Vec<PromptPart>,
    embedded: bool,
) -> Result<(), sdk::Error> {
    let request = sdk::UntypedMessage::new(
        "_session/steering",
        serde_json::json!({
            "sessionId": session,
            "prompt": prompt_blocks(&parts, embedded),
            "_meta": {"steering": {"idleBehavior": "promptRequired"}},
        }),
    )?;
    let request = cx.send_request(request);
    let shared = shared.clone();
    let control = cx.clone();
    let session = session.clone();
    cx.spawn(async move {
        let request = request.block_task();
        futures::pin_mut!(request);
        let response = match with_timeout(request.as_mut(), Duration::from_secs(30)).await {
            Some(response) => response,
            None => {
                shared
                    .emit(AgentEvent::Error {
                        message: "补充指令发送超时，是否已接收尚未确认".into(),
                    })
                    .await;
                // A timeout cannot retract an already sent request. Keep waiting so a
                // late detached start can still be cancelled, without targeting a new turn.
                let stopping = shared.stopping.recv();
                futures::pin_mut!(stopping);
                match futures::future::select(request, stopping).await {
                    Either::Left((response, _)) => response,
                    Either::Right(_) => return Ok(()),
                }
            }
        };
        match response {
            Ok(value) => match value.get("outcome").and_then(serde_json::Value::as_str) {
                Some("injected") => {
                    let end = {
                        let current = shared.turn.lock().unwrap();
                        let mut state = shared.steering.lock().unwrap();
                        if state.pending != Some(turn) {
                            return Ok(());
                        }
                        state.pending = None;
                        if *current != Some(turn) {
                            return Ok(());
                        }
                        state.end_after_request()
                    };
                    if let Some((outcome, force)) = end {
                        shared.end_turn(Some(turn), outcome, force).await;
                    }
                }
                Some("promptRequired") => {
                    let end = {
                        let current = shared.turn.lock().unwrap();
                        let mut state = shared.steering.lock().unwrap();
                        if state.pending != Some(turn) {
                            return Ok(());
                        }
                        state.pending = None;
                        if *current != Some(turn) {
                            return Ok(());
                        }
                        if state.detached && state.idle_seen {
                            state.deferred = Some(TurnOutcome::EndTurn);
                        }
                        if state.cancelled {
                            state.end_after_request()
                        } else {
                            state.followup = Some(parts);
                            None
                        }
                    };
                    if let Some((outcome, force)) = end {
                        shared.end_turn(Some(turn), outcome, force).await;
                    }
                    let _ = shared.turn_done.send(()).await;
                }
                Some("startedNewTurn") => {
                    let (end, cancelled) = {
                        let current = shared.turn.lock().unwrap();
                        let mut state = shared.steering.lock().unwrap();
                        if state.pending != Some(turn) {
                            return Ok(());
                        }
                        if *current != Some(turn) {
                            if state.cancelled {
                                control.send_notification(acp::CancelNotification::new(
                                    session.clone(),
                                ))?;
                            }
                            // Release only after enqueueing cancellation. A new prompt
                            // takes these same locks and cannot receive this late cancel.
                            state.pending = None;
                            return Ok(());
                        }
                        state.pending = None;
                        state.detached = true;
                        state.deferred = None;
                        (
                            (state.active_seen && state.idle_seen).then_some(state.cancelled),
                            state.cancelled,
                        )
                    };
                    if cancelled {
                        control.send_notification(acp::CancelNotification::new(session.clone()))?;
                    }
                    if let Some(cancelled) = end {
                        shared
                            .end_turn(
                                Some(turn),
                                if cancelled {
                                    TurnOutcome::Cancelled
                                } else {
                                    TurnOutcome::EndTurn
                                },
                                true,
                            )
                            .await;
                    }
                }
                _ => failed(&shared, turn, "Agent 未接受补充指令，请重试".into()).await,
            },
            Err(error) => {
                failed(
                    &shared,
                    turn,
                    format!("补充指令发送失败：{}", error_text(&error)),
                )
                .await
            }
        }
        Ok(())
    })
}

pub(super) async fn failed(shared: &SessionState, turn: TurnId, message: String) {
    let end = {
        let current = shared.turn.lock().unwrap();
        let mut state = shared.steering.lock().unwrap();
        if state.pending != Some(turn) {
            return;
        }
        state.pending = None;
        if *current != Some(turn) {
            return;
        }
        state.end_after_request()
    };
    shared.emit(AgentEvent::Error { message }).await;
    if let Some((outcome, force)) = end {
        shared.end_turn(Some(turn), outcome, force).await;
    }
}

pub(super) async fn status(shared: &SessionState, update: &acp::SessionUpdate) {
    let acp::SessionUpdate::SessionInfoUpdate(info) = update else {
        return;
    };
    let Some(status) = info
        .meta
        .as_ref()
        .and_then(|meta| meta.get("codex"))
        .and_then(|codex| codex.get("threadStatus"))
        .and_then(|status| status.get("type"))
        .and_then(serde_json::Value::as_str)
    else {
        return;
    };
    let end = {
        let current = shared.turn.lock().unwrap();
        let Some(turn) = *current else {
            return;
        };
        let mut state = shared.steering.lock().unwrap();
        // The old prompt drains before Codex starts its detached continuation. An old
        // idle notice must not end the new turn before its active notice arrives.
        if status == "active"
            && (state.detached
                || state.deferred.is_some()
                || (state.pending.is_some() && state.idle_seen))
        {
            state.active_seen = true;
            state.idle_seen = false;
        } else if status == "idle" && (state.pending.is_some() || state.active_seen) {
            state.idle_seen = true;
        }
        (state.detached && state.pending.is_none() && state.active_seen && state.idle_seen)
            .then_some((turn, state.cancelled))
    };
    if let Some((turn, cancelled)) = end {
        shared
            .end_turn(
                Some(turn),
                if cancelled {
                    TurnOutcome::Cancelled
                } else {
                    TurnOutcome::EndTurn
                },
                true,
            )
            .await;
        let _ = shared.turn_done.send(()).await;
    }
}
