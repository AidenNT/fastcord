use super::*;

pub(super) const MAX_VOICE_RECONNECT_ATTEMPTS: u8 = 3;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum VoiceRuntimeAction {
    Connect(VoiceGatewaySession),
    Close,
}

struct VoiceRuntimeApplyResult {
    action: Option<VoiceRuntimeAction>,
    participant_playback_changed: bool,
}

#[derive(Default)]
pub(super) struct VoiceRuntimeState {
    current_user_id: Option<Id<UserMarker>>,
    requested: Option<CurrentVoiceConnectionState>,
    current_voice: Option<ObservedSelfVoiceState>,
    server: Option<VoiceServerInfo>,
    active: Option<VoiceGatewaySession>,
    blocked: Option<VoiceGatewaySession>,
    reconnect_target: Option<VoiceGatewaySession>,
    reconnect_attempts: u8,
    push_to_talk: bool,
    push_to_talk_pressed: bool,
    audio_sources: VoiceAudioSources,
    audio_sources_generation: u64,
    participant_playback_settings: HashMap<Id<UserMarker>, VoiceParticipantPlaybackSettings>,
    next_connection_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedSelfVoiceState {
    scope: VoiceScope,
    channel_id: Id<ChannelMarker>,
    session_id: String,
}

impl VoiceRuntimeState {
    #[cfg(test)]
    pub(super) fn apply(&mut self, event: VoiceRuntimeEvent) -> Option<VoiceRuntimeAction> {
        self.apply_with_changes(event).action
    }

    fn apply_with_changes(&mut self, event: VoiceRuntimeEvent) -> VoiceRuntimeApplyResult {
        let mut participant_playback_changed = false;
        match event {
            VoiceRuntimeEvent::Requested(requested) => {
                let target_changed = match (self.requested, requested) {
                    (Some(current), Some(next)) => {
                        current.scope != next.scope || current.channel_id != next.channel_id
                    }
                    (None, None) => false,
                    _ => true,
                };
                if target_changed {
                    self.push_to_talk_pressed = false;
                }
                if requested.is_none() || self.current_voice.is_none() {
                    self.blocked = None;
                }
                if let Some(next) = requested
                    && self.requested.is_some_and(|current| {
                        current.scope != next.scope || current.channel_id != next.channel_id
                    })
                {
                    self.server = None;
                }
                self.requested = requested;
                if self.requested.is_none() {
                    self.current_voice = None;
                    self.server = None;
                    return VoiceRuntimeApplyResult {
                        action: self.close_active(),
                        participant_playback_changed,
                    };
                }
            }
            VoiceRuntimeEvent::ManualRetry(requested) => {
                let target_changed = self.requested.is_none_or(|current| {
                    current.scope != requested.scope || current.channel_id != requested.channel_id
                });
                if target_changed {
                    self.server = None;
                    self.push_to_talk_pressed = false;
                }
                self.requested = Some(requested);
                self.blocked = None;
                self.reconnect_target = None;
                self.reconnect_attempts = 0;
            }
            VoiceRuntimeEvent::AudioSourcesChanged(sources) => {
                if self.audio_sources == sources {
                    return VoiceRuntimeApplyResult {
                        action: None,
                        participant_playback_changed,
                    };
                }
                self.audio_sources = sources;
                self.audio_sources_generation =
                    self.audio_sources_generation.wrapping_add(1).max(1);
                return VoiceRuntimeApplyResult {
                    action: None,
                    participant_playback_changed,
                };
            }
            VoiceRuntimeEvent::AudioSourcesApplyFailed {
                connection_id,
                generation,
                active_sources,
                ..
            } => {
                if self.audio_sources_generation == generation
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.connection_id == connection_id)
                {
                    self.audio_sources = active_sources;
                }
                return VoiceRuntimeApplyResult {
                    action: None,
                    participant_playback_changed,
                };
            }
            #[cfg(feature = "voice-playback")]
            VoiceRuntimeEvent::PushToTalkEnabledChanged(enabled) => {
                if self.push_to_talk != enabled {
                    self.push_to_talk = enabled;
                    self.push_to_talk_pressed = false;
                }
            }
            #[cfg(feature = "voice-playback")]
            VoiceRuntimeEvent::PushToTalkPressed(pressed) => {
                self.push_to_talk_pressed = pressed;
            }
            VoiceRuntimeEvent::ReplaceParticipantPlaybackSettings(settings) => {
                let settings = settings
                    .into_iter()
                    .filter(|(_, settings)| {
                        *settings != VoiceParticipantPlaybackSettings::default()
                    })
                    .collect();
                participant_playback_changed = self.participant_playback_settings != settings;
                self.participant_playback_settings = settings;
            }
            VoiceRuntimeEvent::UpdateParticipantPlaybackSettings { user_id, settings } => {
                if settings == VoiceParticipantPlaybackSettings::default() {
                    participant_playback_changed = self
                        .participant_playback_settings
                        .remove(&user_id)
                        .is_some();
                } else {
                    participant_playback_changed =
                        self.participant_playback_settings.insert(user_id, settings)
                            != Some(settings);
                }
            }
            VoiceRuntimeEvent::CurrentUserReady(user_id) => {
                self.current_user_id = user_id;
            }
            VoiceRuntimeEvent::VoiceState(state) => {
                if let Some(action) = self.record_voice_state(state) {
                    return VoiceRuntimeApplyResult {
                        action: Some(action),
                        participant_playback_changed,
                    };
                }
            }
            VoiceRuntimeEvent::VoiceServer(server) => {
                if server.endpoint.is_none() {
                    self.server = None;
                    return VoiceRuntimeApplyResult {
                        action: self.close_active(),
                        participant_playback_changed,
                    };
                }
                self.server = Some(server);
            }

            VoiceRuntimeEvent::ConnectionEstablished { connection_id } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.connection_id == connection_id)
                {
                    self.reconnect_attempts = 0;
                }
                return VoiceRuntimeApplyResult {
                    action: None,
                    participant_playback_changed,
                };
            }
            VoiceRuntimeEvent::ConnectionEnded {
                connection_id,
                scope,
                channel_id,
                session_id,
                endpoint,
                outcome,
            } => {
                if let Some(active) = self
                    .active
                    .as_ref()
                    .filter(|active| {
                        active.matches_connection_end(
                            connection_id,
                            scope,
                            channel_id,
                            &session_id,
                            &endpoint,
                        )
                    })
                    .cloned()
                {
                    self.active = None;
                    if outcome == VoiceConnectionEnd::Stop {
                        self.blocked = Some(active);
                        return VoiceRuntimeApplyResult {
                            action: None,
                            participant_playback_changed,
                        };
                    }
                    if self.reconnect_attempts >= MAX_VOICE_RECONNECT_ATTEMPTS {
                        self.blocked = Some(active);
                        logging::debug(
                            "voice",
                            format!(
                                "voice reconnect limit reached after {} attempts",
                                MAX_VOICE_RECONNECT_ATTEMPTS
                            ),
                        );
                        return VoiceRuntimeApplyResult {
                            action: None,
                            participant_playback_changed,
                        };
                    }
                    self.reconnect_attempts += 1;
                    return VoiceRuntimeApplyResult {
                        action: self.connect_if_ready(),
                        participant_playback_changed,
                    };
                }
                return VoiceRuntimeApplyResult {
                    action: None,
                    participant_playback_changed,
                };
            }
            VoiceRuntimeEvent::Shutdown => {
                self.push_to_talk_pressed = false;
                return VoiceRuntimeApplyResult {
                    action: self.close_active(),
                    participant_playback_changed,
                };
            }
        }

        VoiceRuntimeApplyResult {
            action: self.connect_if_ready(),
            participant_playback_changed,
        }
    }

    fn record_voice_state(&mut self, state: VoiceStateInfo) -> Option<VoiceRuntimeAction> {
        if self.current_user_id != Some(state.user_id) {
            return None;
        }
        let requested = self.requested?;
        // A leave clears the channel; for a DM that also clears the scope, so we
        // treat any channel-less state for the current user as a disconnect.
        let Some(channel_id) = state.channel_id else {
            self.current_voice = None;
            self.server = None;
            self.push_to_talk_pressed = false;
            return self.close_active();
        };
        if state.scope() != Some(requested.scope) {
            return None;
        }
        let session_id = state
            .session_id
            .filter(|session_id| !session_id.is_empty())?;
        self.current_voice = Some(ObservedSelfVoiceState {
            scope: requested.scope,
            channel_id,
            session_id,
        });
        None
    }

    fn connect_if_ready(&mut self) -> Option<VoiceRuntimeAction> {
        let requested = self.requested?;
        let voice = self.current_voice.as_ref()?;
        if requested.scope != voice.scope || requested.channel_id != voice.channel_id {
            return self.close_active();
        }
        let server = self.server.as_ref()?;
        if server.scope() != Some(requested.scope) {
            return None;
        }
        let endpoint = server.endpoint.as_ref()?.trim_end_matches('/').to_owned();
        if endpoint.is_empty() || server.token.is_empty() {
            return None;
        }
        let mut session = VoiceGatewaySession {
            connection_id: 0,
            scope: requested.scope,
            channel_id: requested.channel_id,
            user_id: self.current_user_id?,
            session_id: voice.session_id.clone(),
            endpoint,
            token: server.token.clone(),
        };
        if self.reconnect_target.as_ref() != Some(&session) {
            self.reconnect_target = Some(session.clone());
            self.reconnect_attempts = 0;
        }
        if self.active.as_ref() == Some(&session) {
            return None;
        }
        if self.blocked.as_ref() == Some(&session) {
            return None;
        }
        self.blocked = None;
        self.next_connection_id = self.next_connection_id.wrapping_add(1).max(1);
        session.connection_id = self.next_connection_id;
        self.active = Some(session.clone());
        Some(VoiceRuntimeAction::Connect(session))
    }

    fn close_active(&mut self) -> Option<VoiceRuntimeAction> {
        self.active.take().map(|_| VoiceRuntimeAction::Close)
    }

    pub(super) fn capture_gate(&self) -> Option<VoiceCaptureGate> {
        let active = self.active.as_ref()?;
        let requested = self.requested?;
        if active.scope != requested.scope || active.channel_id != requested.channel_id {
            return None;
        }
        let capture_enabled = requested.allow_microphone_transmit && !requested.self_mute;
        Some(VoiceCaptureGate {
            transmit_epoch: 0,
            capture_enabled,
            transmit_enabled: capture_enabled && (!self.push_to_talk || self.push_to_talk_pressed),
            use_voice_activity: !self.push_to_talk,
            noise_suppression: requested.noise_suppression,
            microphone_buffer_ms: requested.microphone_buffer_ms,
            microphone_sensitivity: requested.microphone_sensitivity,
            microphone_volume: requested.microphone_volume,
        })
    }

    pub(super) fn playback_gate(&self) -> Option<VoicePlaybackGate> {
        let active = self.active.as_ref()?;
        let requested = self.requested?;
        if active.scope != requested.scope || active.channel_id != requested.channel_id {
            return None;
        }
        Some(VoicePlaybackGate {
            enabled: !requested.self_deaf,
            volume: requested.voice_output_volume,
        })
    }

    pub(super) fn audio_source_selection(&self) -> VoiceAudioSourceSelection {
        VoiceAudioSourceSelection {
            generation: self.audio_sources_generation,
            sources: self.audio_sources.clone(),
        }
    }
}

pub(crate) fn forward_app_event(
    sender: &mpsc::UnboundedSender<VoiceRuntimeEvent>,
    event: &AppEvent,
) {
    let runtime_event = match event {
        AppEvent::Ready(ready) => {
            VoiceRuntimeEvent::CurrentUserReady(state::parse_user_id(&ready.user.id))
        }
        AppEvent::VoiceStateUpdate(state) => match voice_state_info_from_wire(state) {
            Some(info) => VoiceRuntimeEvent::VoiceState(info),
            None => return,
        },
        AppEvent::VoiceServerUpdate(server) => {
            VoiceRuntimeEvent::VoiceServer(VoiceServerInfo {
                guild_id: server.guild_id.as_deref().and_then(state::parse_guild_id),
                channel_id: server
                    .channel_id
                    .as_deref()
                    .and_then(state::parse_channel_id),
                endpoint: server.endpoint.clone(),
                token: server.token.clone(),
            })
        }
        _ => return,
    };
    let _ = sender.send(runtime_event);
}

/// Convierte el `VoiceState` "de cable" (ids como `String`, tal cual lo
/// manda Discord) al `VoiceStateInfo` tipado que usa el resto del
/// subsistema de voz. `None` si el `user_id` no es un snowflake válido
/// (no debería pasar nunca con un payload real).
fn voice_state_info_from_wire(state: &crate::discord::models::VoiceState) -> Option<VoiceStateInfo> {
    let user_id = state::parse_user_id(&state.user_id)?;
    let display_name = state.display_name();
    Some(VoiceStateInfo {
        guild_id: state.guild_id.as_deref().and_then(state::parse_guild_id),
        channel_id: state.channel_id.as_deref().and_then(state::parse_channel_id),
        user_id,
        session_id: state.session_id.clone(),
        member: Some(MemberInfo {
            user_id,
            display_name,
        }),
        deaf: state.deaf,
        mute: state.mute,
        self_deaf: state.self_deaf,
        self_mute: state.self_mute,
    })
}

pub(crate) async fn run_voice_runtime(
    mut events: mpsc::UnboundedReceiver<VoiceRuntimeEvent>,
    events_tx: mpsc::UnboundedSender<VoiceRuntimeEvent>,
    _gateway_commands_tx: mpsc::UnboundedSender<GatewayCommand>,
    status_publisher: VoiceStatusPublisher,
) {
    let mut state = VoiceRuntimeState::default();
    let mut connection_task: Option<JoinHandle<()>> = None;
    let mut connection_session: Option<VoiceGatewaySession> = None;
    let mut audio_sources_tx: Option<watch::Sender<VoiceAudioSourceSelection>> = None;
    let mut capture_gate_tx: Option<mpsc::UnboundedSender<VoiceCaptureGate>> = None;
    let mut playback_gate_tx: Option<mpsc::UnboundedSender<VoicePlaybackGate>> = None;
    let mut participant_playback_tx: Option<
        watch::Sender<HashMap<Id<UserMarker>, VoiceParticipantPlaybackSettings>>,
    > = None;

    while let Some(event) = events.recv().await {
        let shutdown = matches!(event, VoiceRuntimeEvent::Shutdown);
        let changed_audio_sources = matches!(
            &event,
            VoiceRuntimeEvent::AudioSourcesChanged(sources) if state.audio_sources != *sources
        );
        let audio_sources_apply_failure = match &event {
            VoiceRuntimeEvent::AudioSourcesApplyFailed {
                connection_id,
                generation,
                requested_sources,
                active_sources,
                message,
            } if state.audio_sources_generation == *generation
                && state
                    .active
                    .as_ref()
                    .is_some_and(|active| active.connection_id == *connection_id) =>
            {
                Some((
                    requested_sources.clone(),
                    active_sources.clone(),
                    message.clone(),
                ))
            }
            _ => None,
        };
        let VoiceRuntimeApplyResult {
            action,
            participant_playback_changed,
        } = state.apply_with_changes(event);
        if let Some((requested_sources, active_sources, message)) = audio_sources_apply_failure {
            status_publisher
                .publish_audio_sources_apply_failed(requested_sources, active_sources, message)
                .await;
        }
        let connected_this_event = matches!(&action, Some(VoiceRuntimeAction::Connect(_)));
        if let Some(action) = action {
            match action {
                VoiceRuntimeAction::Connect(session) => {
                    if let Some(stopped_session) = stop_voice_connection_task(
                        &mut connection_task,
                        &mut connection_session,
                        &mut audio_sources_tx,
                        &mut capture_gate_tx,
                        &mut playback_gate_tx,
                        "stopping previous voice connection task before reconnect",
                    )
                    .await
                    {
                        status_publisher
                            .publish_speaking(&stopped_session, stopped_session.user_id, false)
                            .await;
                    }
                    let (next_capture_gate_tx, capture_gate_rx) = mpsc::unbounded_channel();
                    let (next_playback_gate_tx, playback_gate_rx) = mpsc::unbounded_channel();
                    let (next_audio_sources_tx, audio_sources_rx) =
                        watch::channel(state.audio_source_selection());
                    let (next_participant_playback_tx, participant_playback_rx) =
                        watch::channel(state.participant_playback_settings.clone());
                    capture_gate_tx = Some(next_capture_gate_tx);
                    playback_gate_tx = Some(next_playback_gate_tx);
                    audio_sources_tx = Some(next_audio_sources_tx);
                    participant_playback_tx = Some(next_participant_playback_tx);
                    let initial_capture_gate = state.capture_gate().unwrap_or(VoiceCaptureGate {
                        transmit_epoch: 0,
                        capture_enabled: false,
                        transmit_enabled: false,
                        use_voice_activity: true,
                        noise_suppression: false,
                        microphone_buffer_ms: None,
                        microphone_sensitivity: MicrophoneSensitivityDb::default(),
                        microphone_volume: VoiceVolumePercent::default(),
                    });
                    let initial_playback_gate =
                        state.playback_gate().unwrap_or(VoicePlaybackGate {
                            enabled: true,
                            volume: VoiceVolumePercent::default(),
                        });
                    connection_session = Some(session.clone());
                    connection_task = Some(tokio::spawn(run_voice_gateway_session(
                        session,
                        events_tx.clone(),
                        status_publisher.clone(),
                        VoiceGatewayControls {
                            audio_sources_rx,
                            initial_capture_gate,
                            capture_gate_rx,
                            initial_playback_gate,
                            playback_gate_rx,
                            participant_playback_rx,
                        },
                    )));
                }
                VoiceRuntimeAction::Close => {
                    if let Some(stopped_session) = stop_voice_connection_task(
                        &mut connection_task,
                        &mut connection_session,
                        &mut audio_sources_tx,
                        &mut capture_gate_tx,
                        &mut playback_gate_tx,
                        "stopping active voice connection task",
                    )
                    .await
                    {
                        status_publisher
                            .publish_speaking(&stopped_session, stopped_session.user_id, false)
                            .await;
                    }
                    participant_playback_tx = None;
                }
            }
        }
        if state.active.is_none() {
            audio_sources_tx = None;
            capture_gate_tx = None;
            playback_gate_tx = None;
            participant_playback_tx = None;
        }
        if let (Some(capture_gate_tx), Some(capture_gate)) =
            (capture_gate_tx.as_ref(), state.capture_gate())
        {
            let _ = capture_gate_tx.send(capture_gate);
        }
        if let (Some(playback_gate_tx), Some(playback_gate)) =
            (playback_gate_tx.as_ref(), state.playback_gate())
        {
            let _ = playback_gate_tx.send(playback_gate);
        }
        if !connected_this_event
            && changed_audio_sources
            && let Some(audio_sources_tx) = audio_sources_tx.as_mut()
        {
            audio_sources_tx.send_replace(state.audio_source_selection());
        }
        if participant_playback_changed
            && !connected_this_event
            && let Some(participant_playback_tx) = participant_playback_tx.as_mut()
        {
            participant_playback_tx.send_replace(state.participant_playback_settings.clone());
        }
        if shutdown {
            break;
        }
    }

    if let Some(stopped_session) = stop_voice_connection_task(
        &mut connection_task,
        &mut connection_session,
        &mut audio_sources_tx,
        &mut capture_gate_tx,
        &mut playback_gate_tx,
        "stopping voice connection task during voice runtime shutdown",
    )
    .await
    {
        status_publisher
            .publish_speaking(&stopped_session, stopped_session.user_id, false)
            .await;
    }
}

pub(super) async fn stop_voice_connection_task(
    connection_task: &mut Option<JoinHandle<()>>,
    connection_session: &mut Option<VoiceGatewaySession>,
    audio_sources_tx: &mut Option<watch::Sender<VoiceAudioSourceSelection>>,
    capture_gate_tx: &mut Option<mpsc::UnboundedSender<VoiceCaptureGate>>,
    playback_gate_tx: &mut Option<mpsc::UnboundedSender<VoicePlaybackGate>>,
    label: &str,
) -> Option<VoiceGatewaySession> {
    audio_sources_tx.take();
    capture_gate_tx.take();
    playback_gate_tx.take();
    let stopped_session = connection_session.take();
    let Some(mut task) = connection_task.take() else {
        return stopped_session;
    };
    logging::debug("voice", label);
    match timeout(VOICE_CONNECTION_SHUTDOWN_TIMEOUT, &mut task).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            logging::debug("voice", format!("voice connection task ended: {error}"));
            stopped_session
        }
        Err(_) => {
            logging::debug("voice", "voice connection graceful stop timed out");
            task.abort();
            let _ = timeout(Duration::from_millis(100), &mut task).await;
            stopped_session
        }
    }
}

