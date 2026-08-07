//! `des::general` mirror: re-export the generic des_engine modules the soccer
//! code imports, plus the local soccer modules.

pub use des_engine::des::general::{
    des_base, general, hungarian, ip_mip_des, lp, lp_des, mpc_point_mass, neural_network, prng, qp,
};

pub mod soccer;
pub mod soccer_elo;
pub mod soccer_eval_gate;
pub mod soccer_genome;
pub mod soccer_lifecycle;
pub mod soccer_rotation;
pub mod tournament;

#[cfg(test)]
mod tests {
    use super::mpc_point_mass::{
        PlanarMpcConfig, PlanarPointMassMpc, PlanarReference, PlanarState,
    };
    use super::neural_network::{
        ActivationName, DenseLayerConfig, FeedForwardNetwork, MaskedPolicyGradientBatchSample,
    };
    use super::qp::{solve_qp_active_set, QPOptions, QPStatus, QuadraticProgram};

    #[test]
    fn super_module_trains_des_masked_policy_minibatch() {
        let mut policy = FeedForwardNetwork::new(vec![DenseLayerConfig {
            weights: vec![vec![0.0], vec![8.0], vec![0.0]],
            biases: vec![0.0, 8.0, 0.0],
            activation: ActivationName::Linear,
        }]);
        let state = [1.0];
        let legal_actions = [true, false, true];
        let before = policy.action_probabilities_masked(&state, &legal_actions);
        let result = policy.train_policy_gradient_batch_masked(
            [MaskedPolicyGradientBatchSample {
                input: &state,
                action: 2,
                legal_action_mask: &legal_actions,
                advantage: 1.0,
            }],
            0.0,
            0.1,
            5.0,
        );
        let after = policy.action_probabilities_masked(&state, &legal_actions);

        assert!(result.update_applied);
        assert_eq!(result.samples_seen, 1);
        assert_eq!(result.samples_applied, 1);
        assert_eq!(after[1], 0.0, "illegal soccer action gained probability");
        assert!(
            after[2] > before[2],
            "reinforced soccer action did not improve"
        );
    }

    #[test]
    fn super_module_runs_des_mpc_with_soccer_safe_acceleration() {
        let config = PlanarMpcConfig {
            horizon: 8,
            a_max: 4.0,
            ..Default::default()
        };
        let mut controller = PlanarPointMassMpc::new(config).expect("valid soccer MPC config");
        let acceleration = controller.control(
            PlanarState {
                pos: [0.0, 0.0],
                vel: [0.0, 0.0],
            },
            &[PlanarReference::through([20.0, 5.0], [6.0, 0.0])],
        );
        let magnitude = acceleration[0].hypot(acceleration[1]);

        assert!(acceleration.iter().all(|component| component.is_finite()));
        assert!(magnitude <= config.a_max + 1e-9);
        assert!(
            acceleration[0] > 0.0,
            "controller did not accelerate upfield"
        );
    }

    #[test]
    fn super_module_solves_des_formation_style_qp() {
        let solution = solve_qp_active_set(
            &QuadraticProgram {
                q: vec![vec![2.0, 0.0], vec![0.0, 2.0]],
                c: vec![-2.0, -5.0],
                a_eq: Some(vec![vec![1.0, 1.0]]),
                b_eq: Some(vec![1.0]),
                lb: Some(vec![Some(0.0), Some(0.0)]),
                ..Default::default()
            },
            QPOptions::default(),
        );

        assert_eq!(solution.status, QPStatus::Optimal);
        assert!((solution.x[0] - 0.0).abs() < 1e-8, "{solution:?}");
        assert!((solution.x[1] - 1.0).abs() < 1e-8, "{solution:?}");
        assert!((solution.x.iter().sum::<f64>() - 1.0).abs() < 1e-8);
    }
}
