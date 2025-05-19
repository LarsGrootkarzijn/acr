use crate::data::PlayerEvent;
use crate::plugins::plugin::Plugin;
use crate::plugins::action_plugin::{ActionPlugin, BaseActionPlugin};
use std::any::Any;
use std::collections::HashSet;
use delegate::delegate;
use log::{trace,debug,warn};
use crate::audiocontrol::AudioController;
use crate::audiocontrol::eventbus::EventBus;
use crate::players::PlayerController;
use std::sync::{Arc, Weak, Mutex};

/// Log level for the EventLogger
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

impl From<&str> for LogLevel {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "debug" => LogLevel::Debug,
            "info" => LogLevel::Info,
            "warning" | "warn" => LogLevel::Warning,
            "error" | "err" => LogLevel::Error,
            _ => LogLevel::Info, // Default to Info for unrecognized values
        }
    }
}

/// A simple plugin that logs player events
pub struct EventLogger {
    /// Base action plugin implementation
    base: BaseActionPlugin,

    /// Whether to only log events from the active player
    only_active: bool,

    /// Log level to use for output
    log_level: LogLevel,

    /// Set of event types to log (if empty, log all events)
    event_types: Option<HashSet<String>>,
    
    /// Subscription to the global event bus
    event_bus_subscription: Arc<Mutex<Option<(u64, crossbeam::channel::Receiver<PlayerEvent>)>>>,
    
    /// Handle to the event listener thread
    event_listener_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl EventLogger {
    /// Create a new EventLogger
    pub fn new(only_active: bool) -> Self {
        Self {
            base: BaseActionPlugin::new("EventLogger"),
            only_active,
            log_level: LogLevel::default(),
            event_types: None,
            event_bus_subscription: Arc::new(Mutex::new(None)),
            event_listener_thread: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new EventLogger with custom configuration
    pub fn with_config(only_active: bool, log_level: LogLevel, event_types: Option<HashSet<String>>) -> Self {
        Self {
            base: BaseActionPlugin::new("EventLogger"),
            only_active,
            log_level,
            event_types,
            event_bus_subscription: Arc::new(Mutex::new(None)),
            event_listener_thread: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the log level
    pub fn set_log_level(&mut self, level: LogLevel) {
        self.log_level = level;
    }

    /// Set the event types to log
    pub fn set_event_types(&mut self, event_types: Option<HashSet<String>>) {
        self.event_types = event_types;
    }

    /// Check if an event type should be logged
    fn should_log_event_type(&self, event_type: &str) -> bool {
        match &self.event_types {
            Some(types) => types.contains(event_type),
            None => true, // Log all event types if none are specified
        }
    }    /// Get the event type name from a PlayerEvent
    fn get_event_type(event: &PlayerEvent) -> &'static str {
        match event {
            PlayerEvent::StateChanged { .. } => "state_changed",
            PlayerEvent::SongChanged { .. } => "song_changed",
            PlayerEvent::LoopModeChanged { .. } => "loop_mode_changed",
            PlayerEvent::RandomChanged { .. } => "random_mode_changed",
            PlayerEvent::CapabilitiesChanged { .. } => "capabilities_changed",
            PlayerEvent::PositionChanged { .. } => "position_changed",
            PlayerEvent::DatabaseUpdating { .. } => "database_updating",
            PlayerEvent::QueueChanged { .. } => "queue_changed",
            PlayerEvent::SongInformationUpdate { .. } => "song_information_update",
            PlayerEvent::ActivePlayerChanged { .. } => "active_player_changed",
        }
    }    
    
    /// Create a handler for events coming from the event bus
    fn handle_event_bus_events(&self, event: PlayerEvent) {
        trace!("Received event");
        // Determine if this is from the active player
        let is_active_player = if let Some(controller) = self.base.get_controller() {
            // Get player ID from the event
            let event_player_id = event.source().player_id();
            
            // Get ID of the active player from AudioController
            let active_player_id = controller.get_player_id();
            
            // Event is from active player if IDs match
            event_player_id == active_player_id
        } else {
            false
        };

        // Log the event the same way as before
        self.log_event(&event, is_active_player);
    }

    /// Log a message with the appropriate log level
    fn log_message(&self, msg: &str, is_active_player: bool) {
        let active_suffix = if is_active_player { " [ACTIVE]" } else { "" };
        let full_msg = format!("{}{}", msg, active_suffix);

        match self.log_level {
            LogLevel::Debug => log::debug!("{}", full_msg),
            LogLevel::Info => log::info!("{}", full_msg),
            LogLevel::Warning => log::warn!("{}", full_msg),
            LogLevel::Error => log::error!("{}", full_msg),
        }
    }

    /// Implementation of the event logging logic
    fn log_event(&self, event: &PlayerEvent, is_active_player: bool) {
        // Only log events from the active player if only_active is true
        if self.only_active && !is_active_player {
            warn!("Should only log events from the active player, but this event is from a different player");
            return;
        }

        // Check if we should log this event type
        let event_type = Self::get_event_type(&event);
        if !self.should_log_event_type(event_type) {
            trace!("Should not log this event type: {}", event_type);
            return;
        }        
        
        match &event {
            PlayerEvent::StateChanged { source, state } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) state changed to {:?}",
                        source.player_name(),
                        source.player_id(),
                        state
                    ),
                    is_active_player
                );
            },
            PlayerEvent::SongChanged { source, song } => {
                if let Some(song) = song {
                    self.log_message(
                        &format!(
                            "Player {} (ID: {}) changed song to \'{}\' by \'{}\'",
                            source.player_name(),
                            source.player_id(),
                            song.title.as_deref().unwrap_or("Unknown"),
                            song.artist.as_deref().unwrap_or("Unknown")
                        ),
                        is_active_player
                    );
                } else {
                    self.log_message(
                        &format!(
                            "Player {} (ID: {}) cleared current song",
                            source.player_name(),
                            source.player_id()
                        ),
                        is_active_player
                    );
                }
            },
            PlayerEvent::LoopModeChanged { source, mode } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) changed loop mode to {:?}",
                        source.player_name(),
                        source.player_id(),
                        mode
                    ),
                    is_active_player
                );
            },
            PlayerEvent::RandomChanged { source, enabled } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) changed random/shuffle mode to {}",
                        source.player_name(),
                        source.player_id(),
                        if *enabled { "enabled" } else { "disabled" }
                    ),
                    is_active_player
                );
            },
            PlayerEvent::CapabilitiesChanged { source, capabilities } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) capabilities changed: {:?}",
                        source.player_name(),
                        source.player_id(),
                        capabilities
                    ),
                    is_active_player
                );
            },
            PlayerEvent::PositionChanged { source, position } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) position changed to {:.1}s",
                        source.player_name(),
                        source.player_id(),
                        position
                    ),
                    is_active_player
                );
            },
            PlayerEvent::DatabaseUpdating { source, artist, album, song, percentage } => {
                let progress_str = if let Some(pct) = percentage {
                    format!(" - {:.1}%", pct)
                } else {
                    String::new()
                };

                let item_str = match (artist, album, song) {
                    (Some(a), Some(b), Some(s)) => format!("artist: {}, album: {}, song: {}", a, b, s),
                    (Some(a), Some(b), None) => format!("artist: {}, album: {}", a, b),
                    (Some(a), None, None) => format!("artist: {}", a),
                    (None, Some(b), None) => format!("album: {}", b),
                    (None, None, Some(s)) => format!("song: {}", s),
                    (None, Some(b), Some(s)) => format!("album: {}, song: {}", b, s),
                    (Some(a), None, Some(s)) => format!("artist: {}, song: {}", a, s),
                    _ => "database".to_string(),
                };

                self.log_message(
                    &format!(
                        "Player {} (ID: {}) updating {}{}",
                        source.player_name(),
                        source.player_id(),
                        item_str,
                        progress_str
                    ),
                    is_active_player
                );
            },
            PlayerEvent::QueueChanged { source } => {
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) queue changed",
                        source.player_name(),
                        source.player_id()
                    ),
                    is_active_player
                );
            },
            PlayerEvent::SongInformationUpdate { source, song } => {
                // song is type Song, not Option<Song>
                self.log_message(
                    &format!(
                        "Player {} (ID: {}) song information updated: \\'{}\\' by \\'{}\\'",
                        source.player_name(),
                        source.player_id(),
                        song.title.as_deref().unwrap_or("Unknown Title"), // Added unwrap_or
                        song.artist.as_deref().unwrap_or("Unknown Artist") // Added unwrap_or
                    ),
                    is_active_player
                );
            },
            PlayerEvent::ActivePlayerChanged { source, player_id } => {
                self.log_message(
                    &format!(
                        "Active player changed from {} (ID: {}) to player ID: {}",
                        source.player_name(),
                        source.player_id(),
                        player_id
                    ),
                    is_active_player
                );
            },
        }
    }    fn get_event_json_payload(&self, event: &PlayerEvent) -> Option<serde_json::Value> {
        match event {
            PlayerEvent::SongChanged { song, .. } => {
                song.as_ref().map(|s| serde_json::json!({
                    "title": s.title,
                    "artist": s.artist,
                    "album": s.album,
                    "stream_url": s.stream_url, // Corrected field name
                    "source": s.source, // Corrected field name
                }))
            }
            PlayerEvent::SongInformationUpdate { song, .. } => {
                // song is type Song
                Some(serde_json::json!({
                    "title": song.title,
                    "artist": song.artist,
                    "album": song.album,
                    "stream_url": song.stream_url, // Corrected field name
                    "source": song.source, // Corrected field name
                }))
            }
            PlayerEvent::StateChanged { state, .. } => Some(serde_json::json!({ "state": state })),
            PlayerEvent::LoopModeChanged { mode, .. } => Some(serde_json::json!({ "loop_mode": mode })),
            PlayerEvent::RandomChanged { enabled, .. } => Some(serde_json::json!({ "random_enabled": enabled })),
            PlayerEvent::CapabilitiesChanged { capabilities, .. } => Some(serde_json::json!({ "capabilities": capabilities })),
            PlayerEvent::PositionChanged { position, .. } => Some(serde_json::json!({ "position": position })),
            PlayerEvent::DatabaseUpdating { artist, album, song, percentage, .. } => {
                let mut payload = serde_json::json!({});

                if let Some(percentage) = percentage {
                    payload["percentage"] = serde_json::json!(percentage);
                }

                match (artist, album, song) {
                    (Some(a), Some(b), Some(s)) => {
                        payload["artist"] = serde_json::json!(a);
                        payload["album"] = serde_json::json!(b);
                        payload["song"] = serde_json::json!(s);
                    },
                    (Some(a), Some(b), None) => {
                        payload["artist"] = serde_json::json!(a);
                        payload["album"] = serde_json::json!(b);
                    },
                    (Some(a), None, None) => {
                        payload["artist"] = serde_json::json!(a);
                    },
                    (None, Some(b), None) => {
                        payload["album"] = serde_json::json!(b);
                    },
                    (None, None, Some(s)) => {
                        payload["song"] = serde_json::json!(s);
                    },
                    (None, Some(b), Some(s)) => {
                        payload["album"] = serde_json::json!(b);
                        payload["song"] = serde_json::json!(s);
                    },
                    (Some(a), None, Some(s)) => {
                        payload["artist"] = serde_json::json!(a);
                        payload["song"] = serde_json::json!(s);
                    },
                    _ => {} // Do nothing for None values
                }

                Some(payload)
            },
            PlayerEvent::QueueChanged { .. } => Some(serde_json::json!({})),
            PlayerEvent::ActivePlayerChanged { player_id, .. } => Some(serde_json::json!({ "player_id": player_id })),
        }
    }
}

impl Plugin for EventLogger {
    delegate! {
        to self.base {
            fn name(&self) -> &str;
            fn version(&self) -> &str;
        }
    }

    fn init(&mut self) -> bool {
        log::info!(
            "EventLogger initialized. Only active: {}, Log level: {:?}, Event types: {:?}",
            self.only_active,
            self.log_level,
            self.event_types
        );
        
        // Set up subscription to the global event bus
        let event_bus = EventBus::instance();
        let (id, receiver) = event_bus.subscribe_all();
        
        // Store our subscription ID (we'll need it to unsubscribe later)
        if let Ok(mut sub) = self.event_bus_subscription.lock() {
            *sub = Some((id, receiver.clone()));
        }

        // Create a thread-safe reference to self for the worker thread
        let event_logger = Arc::new(Mutex::new(self.clone()));
        
        // Start a thread to listen for events from the event bus
        let thread_handle = std::thread::spawn(move || {
            log::debug!("EventLogger event bus listener thread started");
            
            // Process events until the channel is closed
            while let Ok(event) = receiver.recv() {
                // Get a lock on the event logger
                if let Ok(logger) = event_logger.lock() {
                    // Handle the event
                    logger.handle_event_bus_events(event);
                }
            }
            
            log::debug!("EventLogger event bus listener thread exiting");
        });

        // Store the thread handle
        if let Ok(mut handle) = self.event_listener_thread.lock() {
            *handle = Some(thread_handle);
        }
        
        self.base.init()
    }

    fn shutdown(&mut self) -> bool {
        log::info!("EventLogger shutting down");
        
        // Unsubscribe from the event bus
        if let Ok(mut sub_guard) = self.event_bus_subscription.lock() {
            if let Some((id, _)) = sub_guard.take() {
                EventBus::instance().unsubscribe(id);
                log::debug!("EventLogger unsubscribed from event bus");
            }
        }
          // Wait for the event listener thread to exit
        if let Ok(mut thread_guard) = self.event_listener_thread.lock() {
            if thread_guard.is_some() {
                // Just take the handle and drop it, which detaches the thread
                let _ = thread_guard.take();
                log::debug!("EventLogger detaching event bus listener thread");
            }
        }
        
        self.base.shutdown()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ActionPlugin for EventLogger {
    fn initialize(&mut self, controller: Weak<AudioController>) {
        self.base.set_controller(controller);
    }

    fn on_event(&mut self, _event: &PlayerEvent, _is_active_player: bool) {
        // This method is kept for compatibility with the ActionPlugin trait
        // But we're not using it anymore since we're receiving events from the event bus
        // log::trace!("on_event called, but EventLogger is using event bus directly now");
    }
}

// Clone implementation for EventLogger to allow for passing to thread
impl Clone for EventLogger {
    fn clone(&self) -> Self {
        let mut new_base = BaseActionPlugin::new(self.base.name());
        
        // Get the controller reference from the original object
        if let Some(controller) = self.base.get_controller() {
            // The controller is already an Arc, we need to downgrade it to a Weak
            let controller_weak = Arc::downgrade(&controller);
            new_base.set_controller(controller_weak);
        }
        
        Self {
            base: new_base,
            only_active: self.only_active,
            log_level: self.log_level,
            event_types: self.event_types.clone(),
            event_bus_subscription: Arc::new(Mutex::new(None)),
            event_listener_thread: Arc::new(Mutex::new(None)),
        }
    }
}
