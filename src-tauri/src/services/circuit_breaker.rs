use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub timeout: Duration,
    pub error_rate_threshold: f64,
    pub min_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 4,
            success_threshold: 2,
            timeout: Duration::from_secs(60),
            error_rate_threshold: 0.6,
            min_requests: 10,
        }
    }
}

#[derive(Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    total_requests: u32,
    last_failure_at: Option<Instant>,
    last_state_change: Instant,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            total_requests: 0,
            last_failure_at: None,
            last_state_change: Instant::now(),
            config,
        }
    }

    pub fn state(&self) -> &CircuitState {
        &self.state
    }

    pub fn can_execute(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if self.last_state_change.elapsed() >= self.config.timeout {
                    self.transition_to(CircuitState::HalfOpen);
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    pub fn record_success(&mut self) {
        self.total_requests += 1;
        match self.state {
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.transition_to(CircuitState::Closed);
                }
            }
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            _ => {}
        }
    }

    pub fn record_failure(&mut self) {
        self.total_requests += 1;
        self.failure_count += 1;
        self.last_failure_at = Some(Instant::now());

        match self.state {
            CircuitState::HalfOpen => {
                self.transition_to(CircuitState::Open);
            }
            CircuitState::Closed => {
                if self.failure_count >= self.config.failure_threshold {
                    self.transition_to(CircuitState::Open);
                } else if self.total_requests >= self.config.min_requests {
                    let error_rate = self.failure_count as f64 / self.total_requests as f64;
                    if error_rate >= self.config.error_rate_threshold {
                        self.transition_to(CircuitState::Open);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.failure_count
    }

    pub fn reset(&mut self) {
        self.transition_to(CircuitState::Closed);
    }

    fn transition_to(&mut self, new_state: CircuitState) {
        self.state = new_state;
        self.last_state_change = Instant::now();
        self.failure_count = 0;
        self.success_count = 0;
        self.total_requests = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closed_to_open_on_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let mut cb = CircuitBreaker::new(config);

        assert!(cb.can_execute());
        cb.record_failure();
        cb.record_failure();
        assert_eq!(*cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(*cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }

    #[test]
    fn test_half_open_to_closed_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let mut cb = CircuitBreaker::new(config);

        cb.record_failure();
        assert_eq!(*cb.state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(5));
        assert!(cb.can_execute());
        assert_eq!(*cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(*cb.state(), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(*cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_to_open_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let mut cb = CircuitBreaker::new(config);

        cb.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        cb.can_execute();
        assert_eq!(*cb.state(), CircuitState::HalfOpen);

        cb.record_failure();
        assert_eq!(*cb.state(), CircuitState::Open);
    }
}
