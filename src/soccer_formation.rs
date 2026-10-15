use serde_json::{json, Value};

use super::MipProblemSpec;

pub(super) const PITCH_LANES: u8 = 12;
pub(super) const PITCH_ROWS: u8 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Goalkeeper,
    Defender,
    Midfielder,
    Forward,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Goalkeeper => "goalkeeper",
            Role::Defender => "defender",
            Role::Midfielder => "midfielder",
            Role::Forward => "forward",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FormationSlot {
    id: &'static str,
    role: Role,
    lane: u8,
    row: u8,
}

#[derive(Clone, Copy, Debug)]
struct PlayerProfile {
    id: &'static str,
    name: &'static str,
    role: Role,
    quality: u16,
    preferred_lane: u8,
    preferred_row: u8,
}

const F433_SLOTS: [FormationSlot; 11] = [
    FormationSlot {
        id: "gk",
        role: Role::Goalkeeper,
        lane: 6,
        row: 1,
    },
    FormationSlot {
        id: "left_back",
        role: Role::Defender,
        lane: 2,
        row: 5,
    },
    FormationSlot {
        id: "left_center_back",
        role: Role::Defender,
        lane: 4,
        row: 5,
    },
    FormationSlot {
        id: "right_center_back",
        role: Role::Defender,
        lane: 8,
        row: 5,
    },
    FormationSlot {
        id: "right_back",
        role: Role::Defender,
        lane: 10,
        row: 5,
    },
    FormationSlot {
        id: "left_midfield",
        role: Role::Midfielder,
        lane: 3,
        row: 11,
    },
    FormationSlot {
        id: "central_midfield",
        role: Role::Midfielder,
        lane: 6,
        row: 11,
    },
    FormationSlot {
        id: "right_midfield",
        role: Role::Midfielder,
        lane: 9,
        row: 11,
    },
    FormationSlot {
        id: "left_forward",
        role: Role::Forward,
        lane: 2,
        row: 18,
    },
    FormationSlot {
        id: "center_forward",
        role: Role::Forward,
        lane: 6,
        row: 19,
    },
    FormationSlot {
        id: "right_forward",
        role: Role::Forward,
        lane: 10,
        row: 18,
    },
];

const PLAYERS: [PlayerProfile; 15] = [
    PlayerProfile {
        id: "mateo_stone",
        name: "Mateo Stone",
        role: Role::Goalkeeper,
        quality: 92,
        preferred_lane: 6,
        preferred_row: 1,
    },
    PlayerProfile {
        id: "lucas_vale",
        name: "Lucas Vale",
        role: Role::Goalkeeper,
        quality: 78,
        preferred_lane: 6,
        preferred_row: 2,
    },
    PlayerProfile {
        id: "iker_lane",
        name: "Iker Lane",
        role: Role::Defender,
        quality: 88,
        preferred_lane: 2,
        preferred_row: 5,
    },
    PlayerProfile {
        id: "noah_center",
        name: "Noah Center",
        role: Role::Defender,
        quality: 91,
        preferred_lane: 4,
        preferred_row: 5,
    },
    PlayerProfile {
        id: "elias_cover",
        name: "Elias Cover",
        role: Role::Defender,
        quality: 90,
        preferred_lane: 8,
        preferred_row: 5,
    },
    PlayerProfile {
        id: "theo_wide",
        name: "Theo Wide",
        role: Role::Defender,
        quality: 87,
        preferred_lane: 10,
        preferred_row: 5,
    },
    PlayerProfile {
        id: "marco_reserve",
        name: "Marco Reserve",
        role: Role::Defender,
        quality: 75,
        preferred_lane: 6,
        preferred_row: 6,
    },
    PlayerProfile {
        id: "ari_left",
        name: "Ari Left",
        role: Role::Midfielder,
        quality: 89,
        preferred_lane: 3,
        preferred_row: 11,
    },
    PlayerProfile {
        id: "ben_pivot",
        name: "Ben Pivot",
        role: Role::Midfielder,
        quality: 93,
        preferred_lane: 6,
        preferred_row: 11,
    },
    PlayerProfile {
        id: "cam_right",
        name: "Cam Right",
        role: Role::Midfielder,
        quality: 88,
        preferred_lane: 9,
        preferred_row: 11,
    },
    PlayerProfile {
        id: "dani_reserve",
        name: "Dani Reserve",
        role: Role::Midfielder,
        quality: 79,
        preferred_lane: 6,
        preferred_row: 12,
    },
    PlayerProfile {
        id: "leo_left",
        name: "Leo Left",
        role: Role::Forward,
        quality: 91,
        preferred_lane: 2,
        preferred_row: 18,
    },
    PlayerProfile {
        id: "sam_nine",
        name: "Sam Nine",
        role: Role::Forward,
        quality: 94,
        preferred_lane: 6,
        preferred_row: 19,
    },
    PlayerProfile {
        id: "nico_right",
        name: "Nico Right",
        role: Role::Forward,
        quality: 90,
        preferred_lane: 10,
        preferred_row: 18,
    },
    PlayerProfile {
        id: "owen_reserve",
        name: "Owen Reserve",
        role: Role::Forward,
        quality: 81,
        preferred_lane: 6,
        preferred_row: 18,
    },
];

const EXPECTED_PLAYER_BY_SLOT: [usize; 11] = [0, 2, 3, 4, 5, 7, 8, 9, 11, 12, 13];

fn eligible_assignments() -> Vec<(usize, usize)> {
    PLAYERS
        .iter()
        .enumerate()
        .flat_map(|(player_index, player)| {
            F433_SLOTS
                .iter()
                .enumerate()
                .filter_map(move |(slot_index, slot)| {
                    (player.role == slot.role).then_some((player_index, slot_index))
                })
        })
        .collect()
}

fn assignment_score(player: PlayerProfile, slot: FormationSlot) -> f64 {
    let distance = f64::from(
        player.preferred_lane.abs_diff(slot.lane) + player.preferred_row.abs_diff(slot.row),
    );
    f64::from(player.quality) - distance * 0.1
}

pub(super) fn expected_objective() -> f64 {
    F433_SLOTS
        .iter()
        .enumerate()
        .map(|(slot_index, slot)| {
            assignment_score(PLAYERS[EXPECTED_PLAYER_BY_SLOT[slot_index]], *slot)
        })
        .sum()
}

pub(super) fn problem(relaxed: bool) -> MipProblemSpec {
    let assignments = eligible_assignments();
    let variable_count = assignments.len();
    let mut c = vec![0.0; variable_count];
    let ub = vec![1.0; variable_count];
    let mut var_names = vec![String::new(); variable_count];

    for (index, (player_index, slot_index)) in assignments.iter().copied().enumerate() {
        let player = PLAYERS[player_index];
        let slot = F433_SLOTS[slot_index];
        var_names[index] = format!("assign_{}_to_{}", player.id, slot.id);
        c[index] = assignment_score(player, slot);
    }

    let mut a = Vec::with_capacity(PLAYERS.len() + F433_SLOTS.len() * 2);
    let mut b = Vec::with_capacity(a.capacity());
    let mut con_names = Vec::with_capacity(a.capacity());

    for (player_index, player) in PLAYERS.iter().enumerate() {
        let mut row = vec![0.0; variable_count];
        for (index, (candidate_player, _)) in assignments.iter().enumerate() {
            if *candidate_player == player_index {
                row[index] = 1.0;
            }
        }
        a.push(row);
        b.push(1.0);
        con_names.push(format!("player_{}_at_most_one_slot", player.id));
    }

    for (slot_index, slot) in F433_SLOTS.iter().enumerate() {
        let mut upper = vec![0.0; variable_count];
        for (index, (_, candidate_slot)) in assignments.iter().enumerate() {
            if *candidate_slot == slot_index {
                upper[index] = 1.0;
            }
        }
        let lower = upper.iter().map(|coefficient| -*coefficient).collect();
        a.push(upper);
        b.push(1.0);
        con_names.push(format!("slot_{}_at_most_one_player", slot.id));
        a.push(lower);
        b.push(-1.0);
        con_names.push(format!("slot_{}_at_least_one_player", slot.id));
    }

    MipProblemSpec {
        sense: "max".to_string(),
        c,
        a,
        b,
        integer_vars: vec![!relaxed; variable_count],
        ub: Some(ub),
        var_names: Some(var_names),
        con_names: Some(con_names),
    }
}

pub(super) fn decode_assignment(x: &[f64]) -> Result<Vec<usize>, String> {
    let assignments = eligible_assignments();
    let expected_len = assignments.len();
    if x.len() != expected_len {
        return Err(format!(
            "soccer assignment has {} values, expected {expected_len}",
            x.len()
        ));
    }
    F433_SLOTS
        .iter()
        .enumerate()
        .map(|(slot_index, slot)| {
            let selected = assignments
                .iter()
                .enumerate()
                .filter(|(index, (_, candidate_slot))| {
                    *candidate_slot == slot_index && x[*index] > 0.5
                })
                .map(|(_, (player_index, _))| *player_index)
                .collect::<Vec<_>>();
            match selected.as_slice() {
                [player_index] => Ok(*player_index),
                _ => Err(format!(
                    "slot {} has {} selected players",
                    slot.id,
                    selected.len()
                )),
            }
        })
        .collect()
}

fn assignments_document() -> Vec<Value> {
    F433_SLOTS
        .iter()
        .enumerate()
        .map(|(slot_index, slot)| {
            let player = PLAYERS[EXPECTED_PLAYER_BY_SLOT[slot_index]];
            json!({
                "slot": slot.id,
                "role": slot.role.as_str(),
                "lane": slot.lane,
                "row": slot.row,
                "playerId": player.id,
                "playerName": player.name,
            })
        })
        .collect()
}

pub(super) fn model_document(relaxed: bool) -> Value {
    let problem = problem(relaxed);
    json!({
        "requestId": if relaxed { "soccer-f433-lp-ipm" } else { "soccer-f433-binary-mip" },
        "scenario": {
            "source": "Akrion soccer genome F433 anchors",
            "formation": "F433",
            "pitchGrid": {"lanes": PITCH_LANES, "rows": PITCH_ROWS},
            "rosterPlayers": PLAYERS.len(),
            "formationSlots": F433_SLOTS.len(),
            "decisionVariables": problem.c.len(),
            "constraints": problem.a.len(),
            "model": "Assign one eligible roster player to every Akrion formation anchor cell while maximizing player quality and preferred-cell affinity."
        },
        "expected": {
            "status": "optimal",
            "objective": expected_objective(),
            "assignments": assignments_document(),
        },
        "problem": problem,
        "options": {
            "lpAlgorithm": if relaxed { "internal-ipm" } else { "internal-simplex" },
            "lpMaxIters": 5000,
            "splitDepth": 2,
            "maxNodes": 20000,
            "timeoutMs": 120000
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soccer_problem_uses_akrion_grid_and_exact_f433_anchors() {
        let problem = problem(false);
        assert_eq!((PITCH_LANES, PITCH_ROWS), (12, 24));
        assert_eq!(problem.c.len(), 46);
        assert_eq!(problem.a.len(), 37);
        assert_eq!(
            problem.integer_vars.iter().filter(|value| **value).count(),
            46
        );
        assert_eq!(expected_objective(), 993.0);
        assert_eq!(F433_SLOTS[0].lane, 6);
        assert_eq!(F433_SLOTS[0].row, 1);
        assert_eq!(F433_SLOTS[10].lane, 10);
        assert_eq!(F433_SLOTS[10].row, 18);
    }

    #[test]
    fn lp_fixture_only_relaxes_integrality() {
        let mip = problem(false);
        let lp = problem(true);
        assert_eq!(mip.c, lp.c);
        assert_eq!(mip.a, lp.a);
        assert_eq!(mip.b, lp.b);
        assert_eq!(mip.ub, lp.ub);
        assert!(lp.integer_vars.iter().all(|value| !*value));
    }
}
