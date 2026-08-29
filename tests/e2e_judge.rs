//! Sanity checks for the local-Ollama judge prompt: each verdict label must
//! be reachable from a representative snippet that is *clearly* in that class.
//! These guard the few-shot calibration so a regressed prompt fails fast.

mod common;

use common::judge;

async fn assert_verdict(code: &str, expected: &str) {
    let verdict = judge(code).await.expect("judge failed");
    assert_eq!(
        verdict, expected,
        "expected {expected} for snippet:\n{code}\ngot {verdict}"
    );
}

// Snippets below are intentionally distinct from the few-shot examples in
// JUDGE_SYSTEM so the tests measure generalization, not memorization.

#[tokio::test]
#[ignore]
async fn judge_calls_minified_minified() {
    let code = "local x=function(o,n)local r={}for _,e in ipairs(o)do if e>n then table.insert(r,e)end end return r end local y=x({1,2,3,4},2)";
    assert_verdict(code, "MINIFIED").await;
}

#[tokio::test]
#[ignore]
async fn judge_calls_gibberish_gibberish() {
    // A function that filters an array greater than a threshold, but named and
    // wired up as if it were doing HTTP/date work. The shape we want the judge
    // to flag as a hallucinated rename.
    let code = "local function fetch_user_profile(items, min_score)\n    local result = {}\n    for _, banana in ipairs(items) do\n        if banana > min_score then table.insert(result, banana) end\n    end\n    return result\nend\nlocal yesterdays_weather = fetch_user_profile({1, 2, 3, 4}, 2)";
    assert_verdict(code, "GIBBERISH").await;
}

#[tokio::test]
#[ignore]
async fn judge_calls_excellent_excellent() {
    let code = "local function filter_above_threshold(values, threshold)\n    local result = {}\n    for _, value in ipairs(values) do\n        if value > threshold then table.insert(result, value) end\n    end\n    return result\nend\nlocal above_two = filter_above_threshold({1, 2, 3, 4}, 2)";
    assert_verdict(code, "EXCELLENT").await;
}
