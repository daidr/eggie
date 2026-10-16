use eggie_protocol::{AutomationRequest, AutomationResponse, automation_methods};
use serde_json::{Value, json};
use std::io::{self, Read};

pub(super) fn parse(method: &str, params: Value) -> anyhow::Result<AutomationRequest> {
    anyhow::ensure!(params.is_object(), "Parameters must be a JSON object");
    Ok(serde_json::from_value(
        json!({"method":method,"params":params}),
    )?)
}
pub(super) fn call(method: &str, params: Value) -> AutomationResponse {
    match parse(method, params) {
        Ok(request) => eggie_ui::control_running_gui(request).unwrap_or_else(|error| {
            AutomationResponse::error("gui_unavailable", error.to_string())
        }),
        Err(error) => AutomationResponse::error("invalid_params", error.to_string()),
    }
}
pub(super) fn run(args: &[String]) -> i32 {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!(
            "Usage: eggie api <method> ['{{...}}' | -]\n       eggie <projects|tabs|panes|terminal|commands|windows|profiles> <operation> ['{{...}}' | -]\n\nUse - to read JSON parameters from stdin. All responses are JSON. Eggie must already be running.\n"
        );
        for method in automation_methods() {
            println!("  {:<20} {}", method.name, method.description);
        }
        return 0;
    }
    let result = (|| -> anyhow::Result<AutomationResponse> {
        anyhow::ensure!(args.len() <= 2, "Expected one method and one JSON object");
        let params = match args.get(1).map(String::as_str) {
            None => json!({}),
            Some("-") => {
                let mut body = String::new();
                io::stdin().take(64 * 1024 + 1).read_to_string(&mut body)?;
                anyhow::ensure!(body.len() <= 64 * 1024, "Parameters exceed 64 KiB");
                serde_json::from_str(&body)?
            }
            Some(body) => serde_json::from_str(body)?,
        };
        Ok(call(&args[0], params))
    })();
    let response = result
        .unwrap_or_else(|error| AutomationResponse::error("invalid_params", error.to_string()));
    println!("{}", serde_json::to_string(&response).unwrap());
    if matches!(response, AutomationResponse::Ok { .. }) {
        0
    } else {
        1
    }
}
