output "id" {
  description = "Cloud Run v2 service resource ID."
  value       = google_cloud_run_v2_service.this.id
}

output "name" {
  description = "Cloud Run service name."
  value       = google_cloud_run_v2_service.this.name
}

output "uri" {
  description = "Cloud Run service URI."
  value       = google_cloud_run_v2_service.this.uri
}

output "conditions" {
  description = "Observed Cloud Run service conditions for deployment evidence."
  value       = google_cloud_run_v2_service.this.conditions
}
