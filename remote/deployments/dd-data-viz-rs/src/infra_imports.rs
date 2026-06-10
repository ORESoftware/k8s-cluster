use std::collections::BTreeSet;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwsConfigResource {
    pub id: String,
    pub label: String,
    pub resource_type: String,
    pub service: String,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub tag_count: usize,
    pub relationships: Vec<CloudRelationship>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CloudRelationship {
    pub target: String,
    pub relation: String,
}

pub(crate) fn aws_config_resources(items: &[Value]) -> Vec<AwsConfigResource> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| aws_config_resource(item, index))
        .collect()
}

fn aws_config_resource(item: &Value, index: usize) -> AwsConfigResource {
    let resource_id = value_string_any(
        item,
        &["resourceId", "ResourceId", "resourceID", "id", "Id"],
    );
    let arn = value_string_any(item, &["arn", "ARN", "resourceArn", "ResourceArn"]);
    let id = resource_id
        .clone()
        .or_else(|| arn.clone())
        .unwrap_or_else(|| format!("aws-config-resource-{index}"));
    let resource_type = value_string_any(item, &["resourceType", "ResourceType", "type", "Type"])
        .or_else(|| arn.as_deref().map(aws_type_from_arn))
        .unwrap_or_else(|| "AWS::Resource".to_string());
    let service = value_string_any(item, &["service", "Service"])
        .unwrap_or_else(|| aws_service_from_type(&resource_type));
    let label = value_string_any(
        item,
        &[
            "resourceName",
            "ResourceName",
            "name",
            "Name",
            "resourceId",
            "ResourceId",
        ],
    )
    .or_else(|| arn.as_deref().map(resource_tail))
    .unwrap_or_else(|| id.clone());
    let mut relationships = explicit_aws_config_relationships(item);
    relationships.extend(configuration_relationships(item));
    relationships.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.relation.cmp(&right.relation))
    });
    relationships.dedup();

    AwsConfigResource {
        id,
        label,
        resource_type,
        service,
        region: value_string_any(item, &["awsRegion", "AwsRegion", "region", "Region"]),
        zone: value_string_any(
            item,
            &["availabilityZone", "AvailabilityZone", "zone", "Zone"],
        ),
        tag_count: tag_count(item),
        relationships,
    }
}

fn explicit_aws_config_relationships(item: &Value) -> Vec<CloudRelationship> {
    let mut relationships = BTreeSet::new();
    for value in relationship_values(
        item.get("relationships")
            .or_else(|| item.get("Relationships")),
    ) {
        let target = value_string_any(
            value,
            &[
                "resourceId",
                "ResourceId",
                "relatedResourceId",
                "RelatedResourceId",
                "resourceName",
                "ResourceName",
                "arn",
                "ARN",
            ],
        );
        let Some(target) = target else {
            continue;
        };
        let relation = value_string_any(
            value,
            &[
                "relationshipName",
                "RelationshipName",
                "name",
                "Name",
                "relation",
                "Relation",
            ],
        )
        .unwrap_or_else(|| "aws-config-relationship".to_string());
        relationships.insert(CloudRelationship { target, relation });
    }
    relationships.into_iter().collect()
}

fn relationship_values(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(values)) => values.iter().collect(),
        Some(value @ Value::Object(_)) => vec![value],
        _ => Vec::new(),
    }
}

fn configuration_relationships(item: &Value) -> Vec<CloudRelationship> {
    let mut relationships = BTreeSet::new();
    if let Some(configuration) = configuration_value(item) {
        collect_configuration_references(&configuration, &mut relationships);
    }
    if let Some(supplementary) = item
        .get("supplementaryConfiguration")
        .or_else(|| item.get("SupplementaryConfiguration"))
    {
        collect_configuration_references(supplementary, &mut relationships);
    }
    relationships.into_iter().collect()
}

fn configuration_value(item: &Value) -> Option<Value> {
    let value = item
        .get("configuration")
        .or_else(|| item.get("Configuration"))?;
    match value {
        Value::String(raw) => serde_json::from_str(raw).ok(),
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        _ => None,
    }
}

fn collect_configuration_references(value: &Value, output: &mut BTreeSet<CloudRelationship>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if is_aws_reference_key(key) {
                    for target in reference_targets(value) {
                        output.insert(CloudRelationship {
                            target,
                            relation: format!("aws-config:{}", key.trim()),
                        });
                    }
                }
                collect_configuration_references(value, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_configuration_references(value, output);
            }
        }
        _ => {}
    }
}

fn is_aws_reference_key(key: &str) -> bool {
    matches!(
        key,
        "vpcId"
            | "VpcId"
            | "subnetId"
            | "SubnetId"
            | "subnetIds"
            | "SubnetIds"
            | "securityGroupId"
            | "SecurityGroupId"
            | "securityGroupIds"
            | "SecurityGroupIds"
            | "groupId"
            | "GroupId"
            | "groupIds"
            | "GroupIds"
            | "routeTableId"
            | "RouteTableId"
            | "internetGatewayId"
            | "InternetGatewayId"
            | "natGatewayId"
            | "NatGatewayId"
            | "networkInterfaceId"
            | "NetworkInterfaceId"
            | "instanceId"
            | "InstanceId"
            | "roleArn"
            | "RoleArn"
            | "kmsKeyId"
            | "KmsKeyId"
            | "targetGroupArn"
            | "TargetGroupArn"
            | "loadBalancerArn"
            | "LoadBalancerArn"
            | "listenerArn"
            | "ListenerArn"
            | "bucketName"
            | "BucketName"
            | "topicArn"
            | "TopicArn"
            | "queueUrl"
            | "QueueUrl"
            | "functionArn"
            | "FunctionArn"
            | "clusterArn"
            | "ClusterArn"
            | "taskDefinitionArn"
            | "TaskDefinitionArn"
            | "logGroupName"
            | "LogGroupName"
    )
}

fn reference_targets(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => scalar_string(value).into_iter().collect(),
        Value::Array(values) => values.iter().flat_map(reference_targets).collect(),
        Value::Object(map) => [
            "id",
            "Id",
            "resourceId",
            "ResourceId",
            "groupId",
            "GroupId",
            "arn",
            "ARN",
            "name",
            "Name",
        ]
        .iter()
        .filter_map(|key| map.get(*key))
        .flat_map(reference_targets)
        .collect(),
        Value::Number(value) => vec![value.to_string()],
        Value::Bool(value) => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn value_string_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(scalar_value))
}

fn scalar_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => scalar_string(value),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn scalar_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn tag_count(value: &Value) -> usize {
    value
        .get("tags")
        .or_else(|| value.get("Tags"))
        .map(tag_count_value)
        .unwrap_or(0)
}

fn tag_count_value(value: &Value) -> usize {
    match value {
        Value::Object(tags) => tags.len(),
        Value::Array(tags) => tags.len(),
        _ => 0,
    }
}

fn aws_type_from_arn(arn: &str) -> String {
    let service = arn.split(':').nth(2).unwrap_or("Resource");
    let resource = arn.split(':').skip(5).collect::<Vec<_>>().join(":");
    let resource_type = resource
        .split(['/', ':'])
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Resource");
    format!("AWS::{service}::{resource_type}")
}

fn aws_service_from_type(resource_type: &str) -> String {
    let mut parts = resource_type.split("::");
    let vendor = parts.next();
    let service = parts.next();
    if vendor
        .map(|value| value.eq_ignore_ascii_case("aws"))
        .unwrap_or(false)
    {
        return service.unwrap_or("aws").to_ascii_lowercase();
    }
    resource_type
        .split([':', '_', '.'])
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("aws")
        .to_ascii_lowercase()
}

fn resource_tail(value: &str) -> String {
    value
        .rsplit(['/', ':'])
        .find(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aws_config_extracts_explicit_and_configuration_relationships() {
        let resources = aws_config_resources(&[json!({
            "resourceType": "AWS::EC2::Subnet",
            "resourceId": "subnet-123",
            "resourceName": "public",
            "awsRegion": "us-east-1",
            "relationships": [
                {
                    "resourceType": "AWS::EC2::VPC",
                    "resourceId": "vpc-123",
                    "relationshipName": "Is contained in Vpc"
                }
            ],
            "configuration": "{\"vpcId\":\"vpc-123\",\"securityGroupIds\":[\"sg-123\"]}",
            "tags": { "env": "dev" }
        })]);

        assert_eq!(resources[0].id, "subnet-123");
        assert_eq!(resources[0].service, "ec2");
        assert_eq!(resources[0].tag_count, 1);
        assert!(resources[0].relationships.iter().any(|relationship| {
            relationship.target == "vpc-123" && relationship.relation == "Is contained in Vpc"
        }));
        assert!(resources[0].relationships.iter().any(|relationship| {
            relationship.target == "sg-123"
                && relationship.relation == "aws-config:securityGroupIds"
        }));
    }
}
