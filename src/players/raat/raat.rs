use crate::players::player_controller::{BasePlayerController, PlayerController, PlayerStateListener};
use crate::data::{PlayerCapability, PlayerCapabilitySet, Song, LoopMode, PlaybackState, PlayerCommand, PlayerState};
use crate::players::raat::metadata_pipe_reader::MetadataPipeReader;
use crate::data::stream_details::StreamDetails;
use delegate::delegate;
use std::sync::{Arc, Weak, RwLock, Mutex};
use log::{debug, info, warn, error, trace};
use std::thread;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashMap;
use std::any::Any;
use lazy_static::lazy_static;

/// RAAT player controller implementation
/// This controller interfaces with RAAT (Roon Audio Advanced Transport) metadata pipes
pub struct RAATPlayerController {
    /// Base controller for managing state listeners
    base: BasePlayerController,
    
    /// Metadata pipe source path/URL
    metadata_source: String,

    /// Control pipe path/URL for sending commands
    control_pipe: String,
    
    /// Current song information
    current_song: Arc<RwLock<Option<Song>>>,

    /// Current player state
    current_state: Arc<RwLock<PlayerState>>,
    
    /// Current stream details
    stream_details: Arc<RwLock<Option<StreamDetails>>>,
    
    /// Whether to reopen the metadata pipe when it's closed
    reopen_metadata_pipe: bool,
}

// Manually implement Clone for RAATPlayerController
impl Clone for RAATPlayerController {
    fn clone(&self) -> Self {
        RAATPlayerController {
            // Share the BasePlayerController instance to maintain listener registrations
            base: self.base.clone(),
            metadata_source: self.metadata_source.clone(),
            control_pipe: self.control_pipe.clone(),
            current_song: Arc::clone(&self.current_song),
            current_state: Arc::clone(&self.current_state),
            stream_details: Arc::clone(&self.stream_details),
            reopen_metadata_pipe: self.reopen_metadata_pipe,
        }
    }
}

/// Structure to store player state for each instance
struct PlayerInstanceData {
    running_flag: Arc<AtomicBool>
}

/// A map to store running state for each player instance
type PlayerStateMap = HashMap<usize, PlayerInstanceData>;
lazy_static! {
    static ref PLAYER_STATE: Mutex<PlayerStateMap> = Mutex::new(HashMap::new());
}

impl RAATPlayerController {
    /// Create a new RAAT player controller with default settings
    #[allow(dead_code)]
    pub fn new() -> Self {
        debug!("Creating new RAATPlayerController with default settings");
        let source = "/var/run/raat/metadata_pipe"; // Default pipe path
        let control = "/var/run/raat/control_pipe"; // Default control pipe path
        
        // Create a base controller with player name and ID
        let base = BasePlayerController::with_player_info("raat", "raat");
        
        let player = Self {
            base,
            metadata_source: source.to_string(),
            control_pipe: control.to_string(),
            current_song: Arc::new(RwLock::new(None)),
            current_state: Arc::new(RwLock::new(PlayerState::new())),
            stream_details: Arc::new(RwLock::new(None)),
            reopen_metadata_pipe: true,
        };
        
        // Set default capabilities
        player.set_default_capabilities();
        
        player
    }
    
    /// Create a new RAAT player controller with custom metadata source and reopen setting
    #[allow(dead_code)]
    pub fn with_source_and_reopen(source: &str, reopen: bool) -> Self {
        debug!("Creating new RAATPlayerController with source: {} and reopen: {}", source, reopen);
        let control = "/var/run/raat/control_pipe"; // Default control pipe path
        
        // Create a base controller with player name and ID
        let base = BasePlayerController::with_player_info("raat", "raat");
        
        let player = Self {
            base,
            metadata_source: source.to_string(),
            control_pipe: control.to_string(),
            current_song: Arc::new(RwLock::new(None)),
            current_state: Arc::new(RwLock::new(PlayerState::new())),
            stream_details: Arc::new(RwLock::new(None)),
            reopen_metadata_pipe: reopen,
        };
        
        // Set default capabilities
        player.set_default_capabilities();
        
        player
    }

    /// Create a new RAAT player controller with custom metadata source, control pipe, and reopen setting
    pub fn with_pipes_and_reopen(metadata_source: &str, control_pipe: &str, reopen: bool) -> Self {
        debug!("Creating new RAATPlayerController with metadata_source: {}, control_pipe: {}, reopen: {}", 
               metadata_source, control_pipe, reopen);
        
        // Create a base controller with player name and ID
        let base = BasePlayerController::with_player_info("raat", "raat");
        
        let player = Self {
            base,
            metadata_source: metadata_source.to_string(),
            control_pipe: control_pipe.to_string(),
            current_song: Arc::new(RwLock::new(None)),
            current_state: Arc::new(RwLock::new(PlayerState::new())),
            stream_details: Arc::new(RwLock::new(None)),
            reopen_metadata_pipe: reopen,
        };
        
        // Set default capabilities
        player.set_default_capabilities();
        
        player
    }
    
    /// Set the default capabilities for this player
    fn set_default_capabilities(&self) {
        debug!("Setting default RAATPlayerController capabilities");
        
        // We don't actually know what capabilities this player has until we
        // receive metadata, so we'll start with a minimal set and update later
        self.base.set_capabilities(vec![
            PlayerCapability::Play,
            PlayerCapability::Pause,
        ], false); // Don't notify on initialization
    }
    
    /// Update the metadata source
    #[allow(dead_code)]
    pub fn set_metadata_source(&mut self, source: &str) {
        debug!("Updating RAAT metadata source to: {}", source);
        self.metadata_source = source.to_string();
    }
    
    /// Get the current metadata source
    #[allow(dead_code)]
    pub fn get_metadata_source(&self) -> &str {
        &self.metadata_source
    }
    
    /// Set whether to reopen the metadata pipe when it's closed
    #[allow(dead_code)]
    pub fn set_reopen_metadata_pipe(&mut self, reopen: bool) {
        debug!("Setting RAAT metadata pipe reopen to: {}", reopen);
        self.reopen_metadata_pipe = reopen;
    }
    
    /// Get whether the metadata pipe will reopen when closed
    #[allow(dead_code)]
    pub fn get_reopen_metadata_pipe(&self) -> bool {
        self.reopen_metadata_pipe
    }
    
    /// Starts a background thread that listens for RAAT metadata
    /// The thread will run until the running flag is set to false
    fn start_metadata_listener(&self, running: Arc<AtomicBool>, self_arc: Arc<Self>) {
        let source = self.metadata_source.clone();
        
        info!("Starting RAAT metadata listener thread");
        
        // Spawn a new thread for metadata listening
        thread::spawn(move || {
            info!("RAAT metadata listener thread started");
            Self::run_metadata_loop(&source, running, self_arc);
            info!("RAAT metadata listener thread shutting down");
        });
    }

    /// Main event loop for listening to RAAT metadata
    fn run_metadata_loop(source: &str, running: Arc<AtomicBool>, player_arc: Arc<Self>) {
        while running.load(Ordering::SeqCst) {
            // Clone the Arc before moving it into the closure to avoid moving the original
            let player_clone = player_arc.clone();
            
            // Create a metadata callback function that will update the player state
            let callback = Box::new(move |song: Song, state: PlayerState, capabilities: PlayerCapabilitySet, stream_details: StreamDetails| {
                // Process the metadata and update the player
                player_clone.update_metadata(song, state, capabilities, stream_details);
            });
            
            // Create a metadata pipe reader with our callback and reopen setting
            let reader = MetadataPipeReader::with_callback_and_reopen(source, callback, player_arc.reopen_metadata_pipe);
            
            // Try to read from the pipe
            match reader.read_and_log_pipe() {
                Ok(_) => {
                    if player_arc.reopen_metadata_pipe {
                        info!("Metadata pipe closed, will attempt to reconnect");
                    } else {
                        info!("Metadata pipe closed, not reconnecting (reopen=false)");
                        break; // Exit the loop if reopen is false
                    }
                },
                Err(e) => {
                    warn!("Error reading from metadata pipe: {}", e);
                    if !player_arc.reopen_metadata_pipe {
                        warn!("Not reconnecting due to reopen=false");
                        break; // Exit the loop if reopen is false
                    }
                }
            }
            
            // If we get here and reopen is true, wait before trying to reconnect
            if running.load(Ordering::SeqCst) && player_arc.reopen_metadata_pipe {
                info!("Will attempt to reconnect to metadata source in 5 seconds");
                for _ in 0..50 {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            } else {
                // Exit the loop if reopen is false
                break;
            }
        }
    }
    
    /// Process metadata updates from the pipe reader
    fn update_metadata(&self, song: Song, player_state: PlayerState, 
                       capabilities: PlayerCapabilitySet, stream_details: StreamDetails) {
        // Store the new song if different from current
        let mut song_to_notify: Option<Song> = None;
        {
            let mut current_song = self.current_song.write().unwrap();
            let song_changed = match (&*current_song, &song) {
                (Some(old), new) => old.title != new.title || old.artist != new.artist || old.album != new.album,
                (None, _) => true,
            };
            
            if song_changed {
                debug!("Updating current song from metadata");
                // Replace the current song
                *current_song = Some(song.clone());
                song_to_notify = Some(song);
            }
        }
        
        // Check if position has changed and notify if needed
        if let Some(position) = player_state.position {
            // Get the previously stored position
            let old_position = {
                if let Ok(state) = self.current_state.read() {
                    state.position
                } else {
                    None
                }
            };
            
            // If position has changed by more than 1 second or we don't have a previous position, notify
            let position_changed = match old_position {
                Some(old_pos) => (old_pos - position).abs() > 1.0,
                None => true
            };
            
            if position_changed {
                debug!("Position changed to {:.1}s, notifying", position);
                self.base.notify_position_changed(position);
            }
        }
        
        // Update stored player state
        if let Ok(mut current_state) = self.current_state.write() {
            // Update playback state if it has changed
            if current_state.state != player_state.state {
                debug!("Playback state changed from {:?} to {:?}", 
                      current_state.state, player_state.state);
                let new_state = player_state.state;
                current_state.state = new_state;
                
                // Notify listeners of playback state change
                self.base.notify_state_changed(new_state);
            }
            
            // Update position
            if let Some(pos) = player_state.position {
                current_state.position = Some(pos);
            }
            
            // Update loop mode if it has changed
            if current_state.loop_mode != player_state.loop_mode {
                debug!("Loop mode changed from {:?} to {:?}", 
                      current_state.loop_mode, player_state.loop_mode);
                let new_loop_mode = player_state.loop_mode;
                current_state.loop_mode = new_loop_mode;
                
                // Notify listeners of loop mode change
                self.base.notify_loop_mode_changed(new_loop_mode);
            }
            
            // Update shuffle if it has changed
            if current_state.shuffle != player_state.shuffle {
                debug!("Shuffle changed from {} to {}", 
                      current_state.shuffle, player_state.shuffle);
                current_state.shuffle = player_state.shuffle;
            }
            
            // Update metadata
            current_state.metadata = player_state.metadata.clone();
        } else {
            warn!("Failed to acquire lock on current state");
        }
        
        // Update stored capabilities
        let capabilities_changed = self.base.set_capabilities_set(capabilities, false);
        if capabilities_changed {
            let current_caps = self.base.get_capabilities();
            self.base.notify_capabilities_changed(&current_caps);
        }
        
        // Update stored stream details
        if let Ok(mut details) = self.stream_details.write() {
            *details = Some(stream_details);
        }
        
        // Now notify listeners of song change if needed
        // This needs to be done after updating state to avoid race conditions
        if let Some(song) = song_to_notify {
            self.base.notify_song_changed(Some(&song));
        }
        
        // Mark the player as alive since we got data
        self.base.alive();
    }
    
    /// Update the current song and notify listeners (used for testing)
    #[allow(dead_code)]
    pub fn update_current_song(&self, song: Option<Song>) {
        // Store the new song
        if let Ok(mut current_song) = self.current_song.write() {
            let song_changed = match (&*current_song, &song) {
                (Some(old), Some(new)) => old.title != new.title || old.artist != new.artist || old.album != new.album,
                (None, Some(_)) => true,
                (Some(_), None) => true,
                (None, None) => false,
            };
            
            if song_changed {
                debug!("Updating current song");
                // Update the stored song
                *current_song = song.clone();
                
                // Notify listeners of the song change
                drop(current_song); // Release the lock before notifying
                if let Some(s) = &song {
                    self.base.notify_song_changed(Some(s));
                } else {
                    self.base.notify_song_changed(None);
                }
            }
        } else {
            warn!("Failed to acquire write lock for current song");
        }
    }

    /// Write a command to the control pipe
    fn write_to_control_pipe(&self, command: &str) -> bool {
        debug!("Writing command to control pipe: {}", command);
        
        // Use the stream helper to open the control pipe
        // This automatically handles different types of destinations:
        // - Local files/pipes
        // - TCP network streams (using tcp:// URL format)
        // - Windows named pipes or Unix FIFOs
        use crate::helpers::stream_helper::{open_stream, AccessMode};
        
        match open_stream(&self.control_pipe, AccessMode::Write) {
            Ok(mut stream_wrapper) => {
                match stream_wrapper.as_writer() {
                    Ok(writer) => {
                        if let Err(e) = writeln!(writer, "{}", command) {
                            error!("Failed to write command to control pipe: {}", e);
                            false
                        } else {
                            true
                        }
                    },
                    Err(e) => {
                        error!("Failed to get writer from stream: {}", e);
                        false
                    }
                }
            },
            Err(e) => {
                error!("Failed to open control pipe '{}': {}", self.control_pipe, e);
                false
            }
        }
    }

    /// Send a seek command to the control pipe
    fn send_seek_command(&self, position: f64) -> bool {
        debug!("Sending seek command to control pipe: seek to {:.1}s", position);
        self.write_to_control_pipe(&format!("seek {:.1}", position))
    }
}

impl PlayerController for RAATPlayerController {
    delegate! {
        to self.base {
            fn register_state_listener(&mut self, listener: Weak<dyn PlayerStateListener>) -> bool;
            fn unregister_state_listener(&mut self, listener: &Arc<dyn PlayerStateListener>) -> bool;
            fn get_capabilities(&self) -> PlayerCapabilitySet;
            fn get_last_seen(&self) -> Option<std::time::SystemTime>;
        }
    }
    
    fn get_song(&self) -> Option<Song> {
        debug!("Getting current song from stored value");
        // Return a clone of the stored song
        if let Ok(song) = self.current_song.read() {
            song.clone()
        } else {
            warn!("Failed to acquire read lock for current song");
            None
        }
    }
    
    fn get_loop_mode(&self) -> LoopMode {
        debug!("Getting current loop mode");
        // Get the loop mode from the current state
        if let Ok(state) = self.current_state.read() {
            state.loop_mode
        } else {
            warn!("Failed to acquire read lock for current state");
            LoopMode::None
        }
    }
    
    fn get_playback_state(&self) -> PlaybackState {
        trace!("Getting current playback state");
        // Try to get the state from the current state with a timeout
        // Use try_read() to attempt a non-blocking read
        match self.current_state.try_read() {
            Ok(state) => {
                trace!("Got current playback state: {:?}", state.state);
                return state.state;
            },
            Err(_) => {
                // If we can't get a read lock immediately, log a warning
                warn!("Could not acquire immediate read lock for playback state, returning unknown state");
                return PlaybackState::Unknown; // Return a default value if we can't read the state
            }
        }
    }
    
    fn get_position(&self) -> Option<f64> {
        trace!("Getting current playback position");
        // Try to get the position from the current state with a non-blocking read
        match self.current_state.try_read() {
            Ok(state) => {
                trace!("Got current position: {:?}", state.position);
                return state.position;
            },
            Err(_) => {
                warn!("Could not acquire immediate read lock for position, returning None");
                return None; // Return None if we can't read the position
            }
        }
    }
    
    fn get_shuffle(&self) -> bool {
        debug!("Getting current shuffle state");
        if let Ok(state) = self.current_state.read() {
            state.shuffle
        } else {
            warn!("Failed to acquire read lock for current state");
            false
        }
    }
    
    fn get_player_name(&self) -> String {
        "raat".to_string()
    }
    
    fn get_player_id(&self) -> String {
        "raat".to_string()
    }
    
    fn send_command(&self, command: PlayerCommand) -> bool {
        info!("Sending command to RAAT player: {}", command);
        
        // Map the PlayerCommand to the corresponding string command for RAAT
        let cmd_string = match command {
            PlayerCommand::Play => "play",
            PlayerCommand::Pause => "pause",
            PlayerCommand::PlayPause => "playpause",
            PlayerCommand::Next => "next",
            PlayerCommand::Previous => "previous",
            PlayerCommand::Seek(position) => return self.send_seek_command(position),
            PlayerCommand::SetLoopMode(mode) => {
                match mode {
                    LoopMode::None => "loop_off",
                    LoopMode::Track => "loop_track",
                    LoopMode::Playlist => "loop_playlist",
                }
            },
            PlayerCommand::SetRandom(enabled) => {
                if enabled { "shuffle_on" } else { "shuffle_off" }
            },
            PlayerCommand::Kill => "kill",
            PlayerCommand::QueueTracks { .. } => {
                // RAAT doesn't currently support queue operations directly
                warn!("Queue tracks not supported by RAAT player");
                return false;
            },
            PlayerCommand::RemoveTrack(_) => {
                warn!("Remove track not supported by RAAT player");
                return false;
            },
            PlayerCommand::ClearQueue => {
                warn!("Clear queue not supported by RAAT player");
                return false;
            },
        };
        
        // Send the command to the control pipe
        self.write_to_control_pipe(cmd_string)
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn start(&self) -> bool {
        info!("Starting RAAT player controller");
        
        // Create a new Arc<Self> for thread-safe sharing of player instance
        let player_arc = Arc::new(self.clone());
        
        // Create a new running flag
        let running = Arc::new(AtomicBool::new(true));
        
        // Store the running flag in the player instance
        if let Ok(mut state) = PLAYER_STATE.lock() {
            let instance_id = self as *const _ as usize;
            
            if let Some(data) = state.get(&instance_id) {
                // Stop any existing thread
                data.running_flag.store(false, Ordering::SeqCst);
            }
            
            // Start a new listener thread
            self.start_metadata_listener(running.clone(), player_arc.clone());
            
            // Store the running flag
            state.insert(instance_id, PlayerInstanceData { running_flag: running });
            true
        } else {
            error!("Failed to acquire lock for player state");
            false
        }
    }
    
    fn stop(&self) -> bool {
        info!("Stopping RAAT player controller");
        
        // Signal the metadata listener thread to stop
        if let Ok(mut state) = PLAYER_STATE.lock() {
            let instance_id = self as *const _ as usize;
            
            if let Some(data) = state.remove(&instance_id) {
                data.running_flag.store(false, Ordering::SeqCst);
                debug!("Signaled metadata listener thread to stop");
                return true;
            }
        }
        
        debug!("No active metadata listener thread found");
        false
    }
}