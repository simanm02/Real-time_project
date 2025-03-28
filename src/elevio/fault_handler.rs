// Implementation for src/elevio/fault_handler.rs
// This module will handle fault detection and recovery

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use std::thread;

use crate::elevio::system::ElevatorSystem;

// Constants for heartbeat timing
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
// const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);
const ELEVATOR_TIMEOUT: Duration = Duration::from_secs(5); // Time to consider an elevator disconnected

// Structure to track elevator health
pub struct ElevatorHealthMonitor {
    last_seen: HashMap<String, Instant>,
    active_elevators: Arc<Mutex<Vec<String>>>,
}

impl ElevatorHealthMonitor {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            last_seen: HashMap::new(),
            active_elevators: Arc::new(Mutex::new(Vec::new())),
        }))
    }

    // Record that we've seen an elevator
    pub fn record_heartbeat(&mut self, elevator_id: &str) {
        self.last_seen.insert(elevator_id.to_string(), Instant::now());
        
        // Ensure this elevator is in our active list
        let mut active = self.active_elevators.lock().unwrap();
        if !active.contains(&elevator_id.to_string()) {
            active.push(elevator_id.to_string());
            println!("Elevator {} is now active", elevator_id);
        }
    }

    // Check if an elevator is still active
    pub fn is_active(&self, elevator_id: &str) -> bool {
        if let Some(last_seen) = self.last_seen.get(elevator_id) {
            last_seen.elapsed() < ELEVATOR_TIMEOUT
        } else {
            false
        }
    }

    // Get all active elevator IDs
    pub fn get_active_elevators(&self) -> Vec<String> {
        self.last_seen
            .iter()
            .filter(|(_, last_seen)| last_seen.elapsed() < ELEVATOR_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect()
    }

    // Check for and handle disconnected elevators
    pub fn check_disconnected_elevators(&mut self) -> Vec<String> {
        let current_time = Instant::now();
        let mut disconnected = Vec::new();
        
        // Find elevators that haven't been seen recently
        for (id, last_seen) in &self.last_seen {
            if current_time.duration_since(*last_seen) > ELEVATOR_TIMEOUT {
                disconnected.push(id.clone());
            }
        }
        
        // Remove disconnected elevators from our tracking
        for id in &disconnected {
            self.last_seen.remove(id);
            
            // Also remove from active elevators list
            let mut active = self.active_elevators.lock().unwrap();
            if let Some(pos) = active.iter().position(|x| x == id) {
                active.remove(pos);
                println!("Elevator {} disconnected", id);
            }
        }
        
        disconnected
    }
}

// Function to start the health monitoring in a background thread
pub fn start_health_monitoring(
    health_monitor: Arc<Mutex<ElevatorHealthMonitor>>,
    hall_calls: Arc<Mutex<HashMap<(u8, u8), (String, u64)>>>,
    elevator_system: Arc<ElevatorSystem>
) {
    thread::spawn(move || {
        loop {
            // Sleep for a bit before checking
            thread::sleep(HEARTBEAT_INTERVAL);
            
            // Check for disconnected elevators
            let disconnected = {
                let mut monitor = health_monitor.lock().unwrap();
                monitor.check_disconnected_elevators()
            };
            
            // If we found disconnected elevators, reassign their hall calls
            if !disconnected.is_empty() {
                reassign_hall_calls(&disconnected, hall_calls.clone(), Some(elevator_system.clone()));
            }
        }
    });
}

// Function to reassign hall calls from disconnected elevators
fn reassign_hall_calls(
    disconnected_elevators: &[String],
    hall_calls: Arc<Mutex<HashMap<(u8, u8), (String, u64)>>>,
    elevator_system: Option<Arc<ElevatorSystem>>
) {
    let mut calls_to_reassign = Vec::new();
    
    // Find calls assigned to disconnected elevators
    {
        let hall_calls_guard = hall_calls.lock().unwrap();
        for ((floor, direction), (assigned_to, timestamp)) in hall_calls_guard.iter() {
            if disconnected_elevators.contains(assigned_to) {
                calls_to_reassign.push((*floor, *direction, *timestamp));
            }
        }
    }
    
    // Reassign each call
    for (floor, direction, timestamp) in calls_to_reassign {
        println!("Reassigning hall call: floor {}, direction {}", floor, direction);
        
        // Mark the call as unassigned
        {
            let mut hall_calls_guard = hall_calls.lock().unwrap();
            hall_calls_guard.insert((floor, direction), (String::new(), timestamp));
        }
        
        // If we have the elevator system, actively reassign the call
        if let Some(system) = &elevator_system {
            println!("Actively triggering reassignment for call: floor {}, direction {}", 
                     floor, direction);
            system.assign_hall_call(floor, direction, timestamp);
        }
    }
}

// Function to persist elevator state to disk (for recovery after restart)
pub fn persist_elevator_state(
    elevator_id: &str, 
    current_floor: u8,
    current_direction: u8,
    call_buttons: &Vec<Vec<u8>>,
) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write;
    
    // Create a simple state file
    let filename = format!("elevator_{}_state.txt", elevator_id);
    let mut file = File::create(filename)?;
    
    // Write current floor and direction
    writeln!(file, "{}", current_floor)?;
    writeln!(file, "{}", current_direction)?;
    
    // Write call buttons
    for call in call_buttons {
        if call.len() >= 2 {
            writeln!(file, "{},{}", call[0], call[1])?;
        }
    }
    
    Ok(())
}

// Function to load elevator state from disk (for recovery after restart)
pub fn load_elevator_state(elevator_id: &str) -> Option<(u8, u8, Vec<Vec<u8>>)> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    
    let filename = format!("elevator_{}_state.txt", elevator_id);
    match File::open(filename) {
        Ok(file) => {
            let reader = BufReader::new(file);
            let lines: Vec<String> = reader.lines().filter_map(Result::ok).collect();
            
            if lines.len() >= 2 {
                let current_floor = lines[0].parse::<u8>().unwrap_or(0);
                let current_direction = lines[1].parse::<u8>().unwrap_or(0);
                
                let mut call_buttons = Vec::new();
                for i in 2..lines.len() {
                    let parts: Vec<&str> = lines[i].split(',').collect();
                    if parts.len() >= 2 {
                        if let (Ok(floor), Ok(direction)) = (parts[0].parse::<u8>(), parts[1].parse::<u8>()) {
                            call_buttons.push(vec![floor, direction]);
                        }
                    }
                }
                
                return Some((current_floor, current_direction, call_buttons));
            }
            None
        },
        Err(_) => None
    }
}