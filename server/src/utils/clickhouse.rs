//! ClickHouse-specific utility functions

/// Scale factor for Decimal64(6): 10^6
const DECIMAL64_SCALE_6: f64 = 1_000_000.0;

/// Convert f64 to ClickHouse Decimal64(6) representation.
///
/// ClickHouse Decimal64(S) maps to i64 in the `clickhouse` crate,
/// where the value is scaled by 10^S.
pub fn to_decimal64(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    (value * DECIMAL64_SCALE_6).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero() {
        assert_eq!(to_decimal64(0.0), 0);
    }

    #[test]
    fn test_positive() {
        assert_eq!(to_decimal64(1.234567), 1_234_567);
    }

    #[test]
    fn test_small_cost() {
        assert_eq!(to_decimal64(0.000001), 1);
    }

    #[test]
    fn test_rounding() {
        assert_eq!(to_decimal64(0.0000005), 1);
        assert_eq!(to_decimal64(0.0000004), 0);
    }

    #[test]
    fn test_negative() {
        assert_eq!(to_decimal64(-1.234567), -1_234_567);
        assert_eq!(to_decimal64(-0.001), -1000);
    }

    #[test]
    fn test_nan_returns_zero() {
        // Non-finite values are guarded and return 0
        assert_eq!(to_decimal64(f64::NAN), 0);
    }

    #[test]
    fn test_infinity_returns_zero() {
        // Non-finite values are guarded and return 0
        assert_eq!(to_decimal64(f64::INFINITY), 0);
        assert_eq!(to_decimal64(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn test_negative_zero() {
        assert_eq!(to_decimal64(-0.0), 0);
    }

    #[test]
    fn test_typical_llm_costs() {
        // Typical cost range for LLM API calls
        assert_eq!(to_decimal64(0.003456), 3456);
        assert_eq!(to_decimal64(1.50), 1_500_000);
        assert_eq!(to_decimal64(0.0001), 100);
    }
}
