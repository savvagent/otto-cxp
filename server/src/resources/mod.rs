use rmcp::model::*;
use serde_json::json;
use uuid::Uuid;

use crate::tools::CxpServer;

impl CxpServer {
    /// Handle resource reads: customer://profiles/{id}, knowledge://articles/{id}, metrics://dashboard
    pub async fn handle_resource_read(&self, uri: &str) -> Result<String, String> {
        if let Some(id_str) = uri.strip_prefix("customer://profiles/") {
            let id = Uuid::parse_str(id_str).map_err(|e| format!("Invalid UUID: {e}"))?;
            match self.customers.get(id).await {
                Ok(Some(c)) => Ok(serde_json::to_string_pretty(&c).unwrap()),
                Ok(None) => Err("Customer not found".into()),
                Err(e) => Err(format!("Database error: {e}")),
            }
        } else if let Some(id_str) = uri.strip_prefix("knowledge://articles/") {
            let id = Uuid::parse_str(id_str).map_err(|e| format!("Invalid UUID: {e}"))?;
            match self.knowledge.get_article(id).await {
                Ok(Some(a)) => Ok(serde_json::to_string_pretty(&a).unwrap()),
                Ok(None) => Err("Article not found".into()),
                Err(e) => Err(format!("Database error: {e}")),
            }
        } else if uri == "metrics://dashboard" {
            self.get_dashboard_metrics().await
        } else {
            Err(format!("Unknown resource URI: {uri}"))
        }
    }

    async fn get_dashboard_metrics(&self) -> Result<String, String> {
        // Aggregate basic metrics
        let tickets = self
            .tickets
            .list(None, None, None, None, None, None, None, 0, 0)
            .await
            .map_err(|e| format!("Database error: {e}"))?;

        let total_tickets = tickets.len();
        let open_tickets = tickets.iter().filter(|t| t.status == "open").count();
        let in_progress_tickets = tickets
            .iter()
            .filter(|t| t.status == "in_progress")
            .count();
        let resolved_tickets = tickets.iter().filter(|t| t.status == "resolved").count();
        let closed_tickets = tickets.iter().filter(|t| t.status == "closed").count();

        let feedback = self
            .feedback
            .get_feedback(None, None, 1000)
            .await
            .map_err(|e| format!("Database error: {e}"))?;

        let ratings: Vec<i32> = feedback.iter().filter_map(|f| f.rating).collect();
        let csat = crate::analysis::calculate_csat(&ratings);
        let avg_rating = if ratings.is_empty() {
            None
        } else {
            Some(ratings.iter().sum::<i32>() as f64 / ratings.len() as f64)
        };

        let dashboard = json!({
            "totalTickets": total_tickets,
            "openTickets": open_tickets,
            "inProgressTickets": in_progress_tickets,
            "resolvedTickets": resolved_tickets,
            "closedTickets": closed_tickets,
            "csatScore": csat,
            "averageRating": avg_rating,
        });

        Ok(serde_json::to_string_pretty(&dashboard).unwrap())
    }

    pub fn list_resources() -> Vec<Resource> {
        use rmcp::model::RawResource;
        vec![
            Resource {
                raw: RawResource {
                    uri: "customer://profiles/{id}".into(),
                    title: Some("Customer Profile".into()),
                    name: "Customer Profile".into(),
                    description: Some("Get a customer profile by ID".into()),
                    mime_type: Some("application/json".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                annotations: None,
            },
            Resource {
                raw: RawResource {
                    uri: "knowledge://articles/{id}".into(),
                    title: Some("Knowledge Article".into()),
                    name: "Knowledge Article".into(),
                    description: Some("Get a knowledge base article by ID".into()),
                    mime_type: Some("application/json".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                annotations: None,
            },
            Resource {
                raw: RawResource {
                    uri: "metrics://dashboard".into(),
                    title: Some("Metrics Dashboard".into()),
                    name: "Metrics Dashboard".into(),
                    description: Some("Get aggregated CXP metrics".into()),
                    mime_type: Some("application/json".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                annotations: None,
            },
        ]
    }
}
