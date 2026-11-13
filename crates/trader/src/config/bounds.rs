//! The parameters the console may set for a strategy: for each, the lowest and highest value, as
//! the strategy's `bounds` names them in the config. A parameter without bounds is the config's
//! alone. The console reads and sends a strategy's parameters as its `[strategy]` section writes
//! them: `kind`, then each parameter, whole numbers as numbers and decimals as strings.

use std::collections::{BTreeMap, BTreeSet};

use engine_core::Fixed;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use trader_core::strategies::StrategyParams;

/// One parameter's lowest and highest value: whole numbers, or decimals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Bound {
    Whole([u64; 2]),
    Decimal([Fixed; 2]),
}

/// A strategy's bounds, by parameter name.
pub type ParamBounds = BTreeMap<String, Bound>;

/// Why parameters the console asked for are refused: a code a client can match on, and what to
/// show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// `kind_mismatch`, `params_incomplete`, `params_invalid`, `read_only` or `out_of_bounds`.
    pub code: &'static str,
    pub message: String,
}

/// A strategy's parameters as JSON: `kind` and each parameter.
pub fn to_json(params: &StrategyParams) -> Map<String, Value> {
    // A tagged enum of numbers and decimals always serializes, to an object.
    let Ok(Value::Object(map)) = serde_json::to_value(params) else { unreachable!("parameters are an object") };
    map
}

/// Checks `bounds` against the strategy `params` names: each is a parameter of its kind, of the
/// parameter's type, lowest first, and holds the configured value.
/// Error cases:
/// - a name that is not a parameter of the strategy's kind
/// - a whole-number bound for a decimal parameter, or the reverse
/// - a range whose lowest value is above its highest, or one that leaves out the configured value
pub fn check(params: &StrategyParams, bounds: &ParamBounds) -> Result<(), String> {
    let values = to_json(params);
    for (name, bound) in bounds {
        let value = values.get(name).filter(|_| name != "kind");
        let Some(value) = value else {
            let kind = &values["kind"];
            return Err(format!("{name} is not a parameter of {kind}"));
        };
        match within(bound, value) {
            Some(true) => {}
            Some(false) if lowest_first(bound) => return Err(format!("{name}: {bound} leaves out {value}")),
            Some(false) => return Err(format!("{name}: {bound} is not lowest first")),
            None => return Err(format!("{name}: {bound} is not of the parameter's type, as {value} is")),
        }
    }
    Ok(())
}

/// The parameters `body` asks for in place of `current`, when `bounds` lets the console set them:
/// the same kind, every parameter given, each changed one within its bounds, and a strategy its
/// kind takes.
/// Error cases:
/// - `body` names another kind (`kind_mismatch`), leaves out a parameter or adds another
///   (`params_incomplete`), or holds a value of the wrong type (`params_invalid`)
/// - a changed parameter without bounds (`read_only`), or outside them (`out_of_bounds`)
/// - parameters the strategy refuses together (`params_invalid`; see
///   `trader_core::strategy::ParamsError`)
pub fn admit(current: &StrategyParams, body: &Value, bounds: &ParamBounds) -> Result<StrategyParams, Refused> {
    let refused = |code, message| Err(Refused { code, message });
    let now = to_json(current);
    if body.get("kind") != now.get("kind") {
        let kind = body.get("kind").unwrap_or(&Value::Null);
        return refused("kind_mismatch", format!("the strategy is {}, not {kind}", now["kind"]));
    }
    let given: Option<BTreeSet<&String>> = body.as_object().map(|body| body.keys().collect());
    if given != Some(now.keys().collect()) {
        let names: Vec<_> = now.keys().filter(|name| *name != "kind").collect();
        return refused("params_incomplete", format!("give the kind and every parameter: {names:?}"));
    }
    let asked: StrategyParams = match serde_json::from_value(body.clone()) {
        Ok(asked) => asked,
        Err(e) => return refused("params_invalid", e.to_string()),
    };
    let next = to_json(&asked);
    for (name, value) in next.iter().filter(|(name, value)| now.get(*name) != Some(value)) {
        let Some(bound) = bounds.get(name) else {
            return refused("read_only", format!("{name} is not one the console may set"));
        };
        if within(bound, value) != Some(true) {
            return refused("out_of_bounds", format!("{name} must lie within {bound}"));
        }
    }
    match asked.build() {
        Ok(_) => Ok(asked),
        Err(e) => refused("params_invalid", e.to_string()),
    }
}

/// Whether `value` lies within `bound`; `None` when it is not of the bound's type.
fn within(bound: &Bound, value: &Value) -> Option<bool> {
    match (bound, value) {
        (Bound::Whole([low, high]), Value::Number(n)) => n.as_u64().map(|n| (*low..=*high).contains(&n)),
        (Bound::Decimal([low, high]), Value::String(text)) => {
            text.parse::<Fixed>().ok().map(|value| *low <= value && value <= *high)
        }
        _ => None,
    }
}

/// Whether `bound`'s lowest value comes first.
fn lowest_first(bound: &Bound) -> bool {
    match bound {
        Bound::Whole([low, high]) => low <= high,
        Bound::Decimal([low, high]) => low <= high,
    }
}

impl std::fmt::Display for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Whole([low, high]) => write!(f, "[{low}, {high}]"),
            Self::Decimal([low, high]) => write!(f, "[{low}, {high}]"),
        }
    }
}

#[cfg(test)]
mod tests;
