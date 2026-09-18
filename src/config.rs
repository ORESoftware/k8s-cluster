use soccer_engine::soccer::{SoccerLiveServerConfig, SoccerMarlAlgorithm, SoccerNeuralBlendMode};

pub(crate) fn apply_live_learning_env(config: &mut SoccerLiveServerConfig) {
    if let Some(enabled) =
        env_any_bool(&["SOCCER_LIVE_LEARNING_ENABLED", "SOCCER_LEARNING_ENABLED"])
    {
        config.match_config.learning_enabled = enabled;
    }
    if let Some(enabled) = env_any_bool(&[
        "SOCCER_LIVE_LEARNING_LOGGING_ENABLED",
        "SOCCER_LEARNING_LOGGING_ENABLED",
    ]) {
        config.match_config.learning_logging_enabled = enabled;
    }
    if let Some(enabled) = env_any_bool(&[
        "SOCCER_LIVE_FULL_GAME_LEARNING_ENABLED",
        "SOCCER_FULL_GAME_LEARNING_ENABLED",
    ]) {
        config.match_config.full_game_learning_enabled = enabled;
    }
    if let Some(enabled) = env_any_bool(&[
        "SOCCER_LIVE_NEURAL_LEARNING_ENABLED",
        "SOCCER_NEURAL_LEARNING_ENABLED",
    ]) {
        config.match_config.neural_learning.enabled = enabled;
    }
    if let Some(hidden_units) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_HIDDEN_UNITS",
        "SOCCER_NEURAL_HIDDEN_UNITS",
    ]) {
        config.match_config.neural_learning.hidden_units = hidden_units;
    }
    if let Some(learning_rate) = env_any_positive_f64(&[
        "SOCCER_LIVE_NEURAL_LEARNING_RATE",
        "SOCCER_NEURAL_LEARNING_RATE",
        "SOCCER_NEURAL_RATE",
    ]) {
        config.match_config.neural_learning.learning_rate = learning_rate;
    }
    if let Some(batch_size) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_BATCH_SIZE",
        "SOCCER_NEURAL_BATCH_SIZE",
        "SOCCER_NEURAL_LEARNING_BATCH_SIZE",
    ]) {
        config.match_config.neural_learning.batch_size = batch_size;
    }
    if let Some(train_every_ticks) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_TRAIN_EVERY_TICKS",
        "SOCCER_NEURAL_TRAIN_EVERY_TICKS",
        "SOCCER_NEURAL_LEARNING_TRAIN_EVERY_TICKS",
    ]) {
        config.match_config.neural_learning.train_every_ticks = train_every_ticks;
    }
    if let Some(max_batches_per_tick) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_MAX_BATCHES_PER_TICK",
        "SOCCER_NEURAL_MAX_BATCHES_PER_TICK",
        "SOCCER_NEURAL_LEARNING_MAX_BATCHES_PER_TICK",
    ]) {
        config.match_config.neural_learning.max_batches_per_tick = max_batches_per_tick;
    }
    if let Some(max_pending_batches) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_MAX_PENDING_BATCHES",
        "SOCCER_NEURAL_MAX_PENDING_BATCHES",
        "SOCCER_NEURAL_LEARNING_MAX_PENDING_BATCHES",
    ]) {
        config.match_config.neural_learning.max_pending_batches = max_pending_batches;
    }
    if let Some(replay_capacity) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_REPLAY_CAPACITY",
        "SOCCER_NEURAL_REPLAY_CAPACITY",
        "SOCCER_NEURAL_LEARNING_REPLAY_CAPACITY",
    ]) {
        config.match_config.neural_learning.replay_capacity = replay_capacity;
    }
    if let Some(replay_samples_per_tick) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_REPLAY_SAMPLES_PER_TICK",
        "SOCCER_NEURAL_REPLAY_SAMPLES_PER_TICK",
        "SOCCER_NEURAL_LEARNING_REPLAY_SAMPLES_PER_TICK",
    ]) {
        config.match_config.neural_learning.replay_samples_per_tick = replay_samples_per_tick;
    }
    if let Some(snapshot_every_batches) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_SNAPSHOT_EVERY_BATCHES",
        "SOCCER_NEURAL_SNAPSHOT_EVERY_BATCHES",
        "SOCCER_NEURAL_LEARNING_SNAPSHOT_EVERY_BATCHES",
    ]) {
        config.match_config.neural_learning.snapshot_every_batches = snapshot_every_batches;
    }
    if let Some(target_scale) = env_any_positive_f64(&[
        "SOCCER_LIVE_NEURAL_TARGET_SCALE",
        "SOCCER_NEURAL_TARGET_SCALE",
    ]) {
        config.match_config.neural_learning.target_scale = target_scale;
    }
    if let Some(target_clip) = env_any_positive_f64(&[
        "SOCCER_LIVE_NEURAL_TARGET_CLIP",
        "SOCCER_NEURAL_TARGET_CLIP",
    ]) {
        config.match_config.neural_learning.target_clip = target_clip;
    }
    if let Some(mode) =
        env_any_neural_blend_mode(&["SOCCER_LIVE_NEURAL_BLEND_MODE", "SOCCER_NEURAL_BLEND_MODE"])
    {
        config.match_config.neural_blend.mode = mode;
    }
    if let Some(lambda) = env_any_positive_f64(&[
        "SOCCER_LIVE_NEURAL_BLEND_LAMBDA",
        "SOCCER_NEURAL_BLEND_LAMBDA",
    ]) {
        config.match_config.neural_blend.lambda = lambda;
    }
    if let Some(warmup_steps) = env_any_nonnegative_usize(&[
        "SOCCER_LIVE_NEURAL_BLEND_WARMUP_STEPS",
        "SOCCER_NEURAL_BLEND_WARMUP_STEPS",
    ]) {
        config.match_config.neural_blend.warmup_steps = warmup_steps;
    }
    if let Some(candidates) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_BLEND_CANDIDATES",
        "SOCCER_NEURAL_BLEND_CANDIDATES",
    ]) {
        config.match_config.neural_blend.candidates = candidates;
    }
    if let Some(actor_critic) = env_any_bool(&[
        "SOCCER_LIVE_NEURAL_ACTOR_CRITIC",
        "SOCCER_NEURAL_ACTOR_CRITIC",
    ]) {
        config.match_config.neural_blend.actor_critic = actor_critic;
    }
    if let Some(world_model) = env_any_bool(&[
        "SOCCER_LIVE_NEURAL_WORLD_MODEL",
        "SOCCER_NEURAL_WORLD_MODEL",
    ]) {
        config.match_config.neural_blend.world_model = world_model;
    }
    if let Some(marl_algorithm) = env_any_marl_algorithm(&[
        "SOCCER_LIVE_MARL_ALGORITHM",
        "SOCCER_LIVE_NEURAL_MARL_ALGORITHM",
        "SOCCER_MARL_ALGORITHM",
        "SOCCER_NEURAL_MARL_ALGORITHM",
    ]) {
        config.match_config.neural_learning.marl_algorithm = marl_algorithm;
    }
    if let Some(weight) = env_any_positive_f64(&[
        "SOCCER_LIVE_MARL_TEAM_REWARD_WEIGHT",
        "SOCCER_MARL_TEAM_REWARD_WEIGHT",
    ]) {
        config.match_config.neural_learning.marl_team_reward_weight = weight;
    }
    if let Some(weight) = env_any_positive_f64(&[
        "SOCCER_LIVE_MARL_INTERMEDIATE_REWARD_WEIGHT",
        "SOCCER_MARL_INTERMEDIATE_REWARD_WEIGHT",
    ]) {
        config
            .match_config
            .neural_learning
            .marl_intermediate_reward_weight = weight;
    }
    if let Some(epsilon) = env_any_positive_f64(&[
        "SOCCER_LIVE_MAPPO_CLIP_EPSILON",
        "SOCCER_MAPPO_CLIP_EPSILON",
    ]) {
        config.match_config.neural_learning.mappo_clip_epsilon = epsilon;
    }
    if let Some(share) = env_any_positive_f64(&[
        "SOCCER_LIVE_MAPPO_TEAM_REWARD_SHARE",
        "SOCCER_MAPPO_TEAM_REWARD_SHARE",
    ]) {
        config.match_config.neural_learning.mappo_team_reward_share = share;
    }
    if let Some(mcts_enabled) = env_any_bool(&[
        "SOCCER_LIVE_NEURAL_MCTS_ENABLED",
        "SOCCER_NEURAL_MCTS_ENABLED",
    ]) {
        config.match_config.neural_blend.mcts_enabled = mcts_enabled;
    }
    if let Some(mcts_simulations) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_MCTS_SIMULATIONS",
        "SOCCER_NEURAL_MCTS_SIMULATIONS",
    ]) {
        config.match_config.neural_blend.mcts_simulations = mcts_simulations;
    }
    if let Some(mcts_candidates) = env_any_positive_usize(&[
        "SOCCER_LIVE_NEURAL_MCTS_CANDIDATES",
        "SOCCER_NEURAL_MCTS_CANDIDATES",
    ]) {
        config.match_config.neural_blend.mcts_candidates = mcts_candidates;
    }
    if let Some(mcts_depth) =
        env_any_positive_usize(&["SOCCER_LIVE_NEURAL_MCTS_DEPTH", "SOCCER_NEURAL_MCTS_DEPTH"])
    {
        config.match_config.neural_blend.mcts_depth = mcts_depth;
    }
    if let Some(enabled) = env_any_bool(&[
        "SOCCER_LIVE_ADVERSARIAL_EMBEDDING_ENABLED",
        "SOCCER_ADVERSARIAL_EMBEDDING_ENABLED",
    ]) {
        config
            .match_config
            .adversarial_embedding_exploitation_enabled = enabled;
    }
    if let Some(interval) = env_any_positive_usize(&[
        "SOCCER_LIVE_LEARNING_INTERVAL_TICKS",
        "SOCCER_LEARNING_INTERVAL_TICKS",
    ]) {
        config.match_config.learning_interval_ticks = interval;
    }
    if let Some(max_transitions) = env_any_positive_usize(&[
        "SOCCER_LIVE_POLICY_TRAIN_MAX_TRANSITIONS_PER_TICK",
        "SOCCER_POLICY_TRAIN_MAX_TRANSITIONS_PER_TICK",
    ]) {
        config.match_config.policy_train_max_transitions_per_tick = max_transitions;
    }
    if let Some(path) = env_any_string(&["SOCCER_LIVE_POLICY_PATH", "SOCCER_POLICY_PATH"]) {
        config.policy_disk_path = path;
    }
    if let Some(autoload) = env_any_bool(&["SOCCER_LIVE_AUTOLOAD_POLICY", "SOCCER_AUTOLOAD_POLICY"])
    {
        config.autoload_team_policy = autoload;
    }
    if let Some(max_bytes) = env_any_nonnegative_u64(&[
        "SOCCER_LIVE_POLICY_AUTOLOAD_MAX_BYTES",
        "SOCCER_POLICY_AUTOLOAD_MAX_BYTES",
    ]) {
        config.policy_autoload_max_bytes = max_bytes;
    }
    if let Some(autosave) = env_any_bool(&["SOCCER_LIVE_AUTOSAVE_POLICY", "SOCCER_AUTOSAVE_POLICY"])
    {
        config.autosave_team_policy = autosave;
    }
    if let Some(interval) = env_any_positive_u64(&[
        "SOCCER_LIVE_POLICY_AUTOSAVE_INTERVAL_TICKS",
        "SOCCER_POLICY_AUTOSAVE_INTERVAL_TICKS",
    ]) {
        config.policy_autosave_interval_ticks = interval;
    }
    if let Some(keep_best) =
        env_any_bool(&["SOCCER_LIVE_POLICY_KEEP_BEST", "SOCCER_POLICY_KEEP_BEST"])
    {
        config.policy_keep_best = keep_best;
    }
}

fn env_any_bool(names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| {
        let value = std::env::var(name).ok()?;
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

fn env_any_positive_usize(names: &[&str]) -> Option<usize> {
    names.iter().find_map(|name| {
        let value = std::env::var(name).ok()?;
        value
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
    })
}

fn env_any_nonnegative_usize(names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok()?.trim().parse::<usize>().ok())
}

fn env_any_positive_f64(names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| {
        let value = std::env::var(name).ok()?;
        value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
    })
}

fn env_any_neural_blend_mode(names: &[&str]) -> Option<SoccerNeuralBlendMode> {
    names.iter().find_map(|name| {
        let value = std::env::var(name).ok()?;
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" => Some(SoccerNeuralBlendMode::Off),
            "additive" | "add" => Some(SoccerNeuralBlendMode::Additive),
            "tiebreak" | "tie" | "tie-break" | "tie_break" => Some(SoccerNeuralBlendMode::TieBreak),
            "confidence" | "confidencegated" | "confidence-gated" | "gated" => {
                Some(SoccerNeuralBlendMode::ConfidenceGated)
            }
            "authoritative" | "neural" | "neural-authoritative" | "neural_authoritative" => {
                Some(SoccerNeuralBlendMode::Authoritative)
            }
            _ => None,
        }
    })
}

fn env_any_marl_algorithm(names: &[&str]) -> Option<SoccerMarlAlgorithm> {
    names
        .iter()
        .find_map(|name| parse_marl_algorithm(&std::env::var(name).ok()?))
}

fn parse_marl_algorithm(value: &str) -> Option<SoccerMarlAlgorithm> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| *ch != '-' && *ch != '_')
        .collect::<String>();
    match normalized.as_str() {
        "off" | "disabled" | "none" => Some(SoccerMarlAlgorithm::Off),
        "independentactorcritic" | "independent" | "actorcritic" => {
            Some(SoccerMarlAlgorithm::IndependentActorCritic)
        }
        "mappo" => Some(SoccerMarlAlgorithm::Mappo),
        _ => None,
    }
}

fn env_any_string(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let value = std::env::var(name).ok()?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn env_any_nonnegative_u64(names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok()?.trim().parse::<u64>().ok())
}

fn env_any_positive_u64(names: &[&str]) -> Option<u64> {
    env_any_nonnegative_u64(names).filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_marl_algorithm_aliases() {
        assert_eq!(
            parse_marl_algorithm("mappo"),
            Some(SoccerMarlAlgorithm::Mappo)
        );
        assert_eq!(
            parse_marl_algorithm("independent_actor_critic"),
            Some(SoccerMarlAlgorithm::IndependentActorCritic)
        );
        assert_eq!(parse_marl_algorithm("off"), Some(SoccerMarlAlgorithm::Off));
        assert_eq!(parse_marl_algorithm("wat"), None);
    }
}
