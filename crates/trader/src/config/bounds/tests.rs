use serde_json::json;

use super::*;

/// A Bollinger reversion of period 20 and width 2, as a config writes it.
fn bollinger() -> StrategyParams {
    serde_json::from_value(json!({ "kind": "bollinger_reversion", "period": 20, "width": "2" })).unwrap()
}

/// Bounds as a config writes them.
fn bounds(value: serde_json::Value) -> ParamBounds {
    serde_json::from_value(value).unwrap()
}

/// The parameters serve as the config writes them: whole numbers as numbers, decimals as strings.
#[test]
fn parameters_read_as_the_config_writes_them() {
    assert_eq!(
        Value::Object(to_json(&bollinger())),
        json!({ "kind": "bollinger_reversion", "period": 20, "width": "2" })
    );
    let both = bounds(json!({ "period": [10, 30], "width": ["1.5", "3"] }));
    assert_eq!(serde_json::to_value(&both).unwrap(), json!({ "period": [10, 30], "width": ["1.5", "3"] }));
}

/// Bounds name the kind's parameters, of their types, lowest first, holding the configured values.
#[test]
fn bounds_are_checked_against_the_strategy() {
    let params = bollinger();
    assert_eq!(check(&params, &bounds(json!({ "period": [10, 30], "width": ["1.5", "3"] }))), Ok(()));
    for (bad, error) in [
        (json!({ "fast": [1, 5] }), "fast is not a parameter of \"bollinger_reversion\""),
        (json!({ "kind": [1, 5] }), "kind is not a parameter of \"bollinger_reversion\""),
        (json!({ "period": ["10", "30"] }), "period: [10, 30] is not of the parameter's type, as 20 is"),
        (json!({ "width": [1, 3] }), "width: [1, 3] is not of the parameter's type, as \"2\" is"),
        (json!({ "period": [30, 10] }), "period: [30, 10] is not lowest first"),
        (json!({ "period": [21, 30] }), "period: [21, 30] leaves out 20"),
        (json!({ "width": ["2.5", "3"] }), "width: [2.5, 3] leaves out \"2\""),
    ] {
        assert_eq!(check(&params, &bounds(bad.clone())), Err(error.to_owned()), "{bad}");
    }
}

/// A change takes the same kind, every parameter, and changes only bounded parameters, within their
/// bounds, to a strategy its kind takes; an unchanged parameter needs no bounds.
#[test]
fn a_change_stays_within_the_bounds() {
    let (params, limits) = (bollinger(), bounds(json!({ "width": ["1", "3"] })));
    let asked = |body| admit(&params, &body, &limits);
    let wider = asked(json!({ "kind": "bollinger_reversion", "period": 20, "width": "2.5" })).unwrap();
    assert_eq!(Value::Object(to_json(&wider))["width"], "2.5");
    // The same value written another way is no change, a whole number for a decimal too.
    let two = json!({ "kind": "bollinger_reversion", "period": 20, "width": 2 });
    assert_eq!(asked(two), Ok(params.clone()));
    assert_eq!(asked(json!({ "kind": "bollinger_reversion", "period": 20, "width": "2.0" })), Ok(params.clone()));
    for (body, code, error) in [
        (
            json!({ "kind": "rsi_reversion", "period": 20 }),
            "kind_mismatch",
            "the strategy is \"bollinger_reversion\", not \"rsi_reversion\"",
        ),
        (json!({ "period": 20, "width": "2" }), "kind_mismatch", "the strategy is \"bollinger_reversion\", not null"),
        (
            json!({ "kind": "bollinger_reversion", "width": "2" }),
            "params_incomplete",
            "give the kind and every parameter: [\"period\", \"width\"]",
        ),
        (
            json!({ "kind": "bollinger_reversion", "period": 20, "width": "2", "bar": "1h" }),
            "params_incomplete",
            "give the kind and every parameter: [\"period\", \"width\"]",
        ),
        (
            json!({ "kind": "bollinger_reversion", "period": 25, "width": "2" }),
            "read_only",
            "period is not one the console may set",
        ),
        (
            json!({ "kind": "bollinger_reversion", "period": 20, "width": "3.5" }),
            "out_of_bounds",
            "width must lie within [1, 3]",
        ),
    ] {
        assert_eq!(asked(body.clone()).map_err(|e| (e.code, e.message)), Err((code, error.to_owned())), "{body}");
    }
    // Within its bounds, a value the strategy refuses with the others is refused.
    let sma: StrategyParams =
        serde_json::from_value(json!({ "kind": "sma_crossover", "fast": 5, "slow": 20 })).unwrap();
    let periods = bounds(json!({ "fast": [1, 30], "slow": [10, 40] }));
    let refused = admit(&sma, &json!({ "kind": "sma_crossover", "fast": 25, "slow": 20 }), &periods);
    let refused = refused.unwrap_err();
    assert_eq!(refused.code, "params_invalid");
    assert!(refused.message.starts_with("the fast period must be at least 1 and below the slow period"));
}
