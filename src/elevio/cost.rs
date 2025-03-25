use crate::elevio::elev::{DIRN_UP, DIRN_DOWN, DIRN_STOP};

// Simple cost calculation for elevator assignment
pub fn calculate_cost(
    current_floor: u8, 
    current_direction: u8, 
    call_buttons_len: usize,
    call_floor: u8, 
    call_direction: u8
) -> i32 {
    let mut cost = 0;
    
    // Base cost is the distance
    let floor_distance = if current_floor > call_floor {
        (current_floor - call_floor) as i32
    } else {
        (call_floor - current_floor) as i32
    };
    
    cost += floor_distance * 10;
    
    // Add cost for each pending call
    cost += call_buttons_len as i32 * 5;
    
    // Bonus if elevator is idle
    if current_direction == DIRN_STOP {
        cost -= 10;
    }
    
    // Bonus if elevator is already moving in the right direction
    if (current_direction == DIRN_UP && call_floor > current_floor && call_direction == DIRN_UP) ||
       (current_direction == DIRN_DOWN && call_floor < current_floor && call_direction == DIRN_DOWN) {
        cost -= 20;
    }
    
    // Penalty if elevator would need to reverse direction
    if (current_direction == DIRN_UP && call_floor < current_floor) ||
       (current_direction == DIRN_DOWN && call_floor > current_floor) {
        cost += 30;
    }
    
    cost
}

/// Message types for elevator network communication
#[derive(Debug, Clone)]
pub enum ElevatorMessage {
    /// Message for a new hall call
    HallCall { 
        floor: u8, 
        direction: u8, 
        timestamp: u64 
    },
    
    /// Message containing an elevator's current state
    ElevatorState { 
        id: String, 
        floor: u8, 
        direction: u8, 
        call_buttons: Vec<Vec<u8>> 
    },
    
    /// Message indicating a call has been completed
    CompletedCall { 
        floor: u8, 
        direction: u8 
    },

    SyncRequest {
        id: String,
    },
}

impl ElevatorMessage {
    // Convert message to string for network transmission
    pub fn to_string(&self) -> String {
        match self {
            ElevatorMessage::HallCall { floor, direction, timestamp } => {
                format!("HALL|{}|{}|{}", floor, direction, timestamp)
            },
            ElevatorMessage::ElevatorState { id, floor, direction, call_buttons } => {
                // Format call buttons as a compact string
                let buttons_str = call_buttons.iter()
                    .map(|call| format!("{},{}", call[0], call[1]))
                    .collect::<Vec<String>>()
                    .join(";");
                
                format!("STATE|{}|{}|{}|{}", id, floor, direction, buttons_str)
            },
            ElevatorMessage::CompletedCall { floor, direction } => {
                format!("COMPLETED|{}|{}", floor, direction)
            },
            ElevatorMessage::SyncRequest { id } => {
                format!("SYNC|{}", id)
            },
        }
    }
    
    // Parse string back to message
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        
        if parts.is_empty() {
            return None;
        }
        
        match parts[0] {
            "HALL" => {
                if parts.len() < 4 {
                    return None;
                }
                
                let floor = parts[1].parse::<u8>().ok()?;
                let direction = parts[2].parse::<u8>().ok()?;
                let timestamp = parts[3].parse::<u64>().ok()?;
                
                Some(ElevatorMessage::HallCall { floor, direction, timestamp })
            },
            "STATE" => {
                if parts.len() < 5 {
                    return None;
                }
                
                let id = parts[1].to_string();
                let floor = parts[2].parse::<u8>().ok()?;
                let direction = parts[3].parse::<u8>().ok()?;
                
                // Parse call buttons
                let mut call_buttons = Vec::new();
                if !parts[4].is_empty() {
                    for button_str in parts[4].split(';') {
                        let button_parts: Vec<&str> = button_str.split(',').collect();
                        if button_parts.len() == 2 {
                            if let (Ok(f), Ok(d)) = (button_parts[0].parse::<u8>(), button_parts[1].parse::<u8>()) {
                                call_buttons.push(vec![f, d]);
                            }
                        }
                    }
                }
                
                Some(ElevatorMessage::ElevatorState { id, floor, direction, call_buttons })
            },
            "COMPLETED" => {
                if parts.len() < 3 {
                    return None;
                }
                
                let floor = parts[1].parse::<u8>().ok()?;
                let direction = parts[2].parse::<u8>().ok()?;
                
                Some(ElevatorMessage::CompletedCall { floor, direction })
            },
            "SYNC" => {
                if parts.len() < 2 {
                    return None;
                }

                let id = parts[1].to_string();
                Some(ElevatorMessage::SyncRequest {id})
            },
            _ => None,
        }
    }
}