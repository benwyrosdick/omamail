//! Jev classification. One Choice question per message, against the user's
//! category list. The API key is passed in and never logged.

use serde::Deserialize;
use serde_json::{json, Value};

pub const MODEL: &str = "jev-latest";
pub const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Category {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub color: String,
}

pub fn default_categories() -> Vec<Category> {
    [
        (
            "Receipts",
            "Orders, invoices, payment confirmations, and shipping updates for something you bought",
        ),
        (
            "Marketing",
            "Newsletters, promotions, sales, and bulk mail you did not ask a person for",
        ),
        (
            "Personal",
            "Mail from a person to you, not a company broadcast",
        ),
        (
            "Work",
            "Colleagues, meetings, projects, and workplace tools",
        ),
        (
            "Finance",
            "Banks, bills, statements, taxes, and investments that are not a simple receipt",
        ),
        (
            "Travel",
            "Flights, hotels, trains, and reservations",
        ),
        (
            "Social",
            "Social networks, comments, follows, and event invitations",
        ),
        (
            "Other",
            "Nothing else fits",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (name, description))| Category {
        color: color_for_name(name, index),
        name: name.into(),
        description: description.into(),
    })
    .collect()
}

pub fn color_for_name(name: &str, index: usize) -> String {
    match name.to_ascii_lowercase().as_str() {
        "receipts" => "#a6e3a1",
        "marketing" => "#f9e2af",
        "personal" => "#89b4fa",
        "work" => "#cba6f7",
        "finance" => "#f38ba8",
        "travel" => "#94e2d5",
        "social" => "#fab387",
        "other" => "#a6adc8",
        _ => {
            const FALLBACK: [&str; 8] = [
                "#a6e3a1", "#f9e2af", "#89b4fa", "#cba6f7", "#f38ba8", "#94e2d5", "#fab387",
                "#a6adc8",
            ];
            FALLBACK[index % FALLBACK.len()]
        }
    }
    .into()
}

pub fn normalize_color(color: &str, name: &str, index: usize) -> String {
    let trimmed = color.trim();
    if trimmed.len() == 7 && trimmed.starts_with('#') && trimmed[1..].chars().all(|ch| ch.is_ascii_hexdigit())
    {
        trimmed.to_ascii_lowercase()
    } else {
        color_for_name(name, index)
    }
}

pub fn choice_request(from: &str, subject: &str, snippet: &str, categories: &[Category]) -> Value {
    let mut criteria = serde_json::Map::new();
    for category in categories {
        criteria.insert(category.name.clone(), json!(category.description));
    }
    json!({
        "model": MODEL,
        "state": {
            "from": from,
            "subject": subject,
            "snippet": snippet
        },
        "questions": {
            "category": {
                "type": "choice",
                "instructions": "Which category best fits this email? Judge the sender, subject, and snippet together. Pick the closest category. Use Other when nothing more specific fits.",
                "criteria": criteria
            }
        }
    })
}

#[derive(Deserialize)]
struct ChoiceAnswer {
    choice: String,
    confidence: Option<f64>,
}

pub fn parse_choice(body: &str) -> anyhow::Result<(String, Option<f64>)> {
    let value: Value = serde_json::from_str(body).context_parse()?;
    let answer = value
        .pointer("/answers/category")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Jev response had no category answer"))?;
    let parsed: ChoiceAnswer =
        serde_json::from_value(answer).map_err(|err| anyhow::anyhow!("Jev choice was not usable: {err}"))?;
    Ok((parsed.choice, parsed.confidence))
}

trait ParseContext<T> {
    fn context_parse(self) -> anyhow::Result<T>;
}

impl<T> ParseContext<T> for Result<T, serde_json::Error> {
    fn context_parse(self) -> anyhow::Result<T> {
        self.map_err(|err| anyhow::anyhow!("Jev response was not JSON: {err}"))
    }
}

pub async fn classify(
    http: &reqwest::Client,
    api_key: &str,
    from: &str,
    subject: &str,
    snippet: &str,
    categories: &[Category],
) -> anyhow::Result<(String, Option<f64>)> {
    let response = http
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .json(&choice_request(from, subject, snippet, categories))
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("Jev unreachable: {err}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| anyhow::anyhow!("Jev response unreadable: {err}"))?;
    if !status.is_success() {
        anyhow::bail!("Jev rejected the request ({}): {}", status.as_u16(), truncate(&body));
    }
    let (choice, confidence) = parse_choice(&body)?;
    if !categories.iter().any(|category| category.name == choice) {
        anyhow::bail!("Jev returned a category that is not in the list");
    }
    Ok((choice, confidence))
}

fn truncate(text: &str) -> String {
    text.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_request_uses_the_edited_list_and_hides_the_key() {
        let categories = vec![
            Category {
                name: "Receipts".into(),
                description: "Orders and invoices".into(),
                color: "#a6e3a1".into(),
            },
            Category {
                name: "Other".into(),
                description: "Nothing else fits".into(),
                color: "#a6adc8".into(),
            },
        ];
        let request = choice_request("Shop <hi@shop.test>", "Your order", "Thanks for buying", &categories);
        assert_eq!(request["model"], "jev-latest");
        assert_eq!(request["questions"]["category"]["type"], "choice");
        assert_eq!(
            request["questions"]["category"]["criteria"]["Receipts"],
            "Orders and invoices"
        );
        let rendered = request.to_string();
        assert!(!rendered.contains("sk-"));
        assert!(!rendered.to_ascii_lowercase().contains("api_key"));
    }

    #[test]
    fn choice_response_parses() {
        let body = r#"{"model":"jev-latest","answers":{"category":{"type":"choice","choice":"Receipts","confidence":0.8,"probabilities":{"Receipts":0.8,"Other":0.2}}}}"#;
        let (choice, confidence) = parse_choice(body).unwrap();
        assert_eq!(choice, "Receipts");
        assert_eq!(confidence, Some(0.8));
    }
}
