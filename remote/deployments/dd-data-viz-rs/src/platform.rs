use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductParity {
    pub product: &'static str,
    pub category: &'static str,
    pub parity_goal: &'static str,
    pub implemented_surfaces: Vec<&'static str>,
    pub next_engine_work: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDescriptor {
    pub id: &'static str,
    pub family: &'static str,
    pub mode: &'static str,
    pub auth: &'static str,
    pub planner_notes: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticModel {
    pub id: &'static str,
    pub lineage: &'static str,
    pub dimensions: Vec<DimensionDefinition>,
    pub measures: Vec<MeasureDefinition>,
    pub calculations: Vec<CalculationDefinition>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionDefinition {
    pub name: &'static str,
    pub kind: &'static str,
    pub sql: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasureDefinition {
    pub name: &'static str,
    pub aggregate: &'static str,
    pub expression: &'static str,
    pub dax_analog: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculationDefinition {
    pub name: &'static str,
    pub expression_language: &'static str,
    pub expression: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbookBlueprint {
    pub id: &'static str,
    pub persona: &'static str,
    pub grid_model: &'static str,
    pub interactions: Vec<&'static str>,
    pub live_data_strategy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EtlPrimitive {
    pub id: &'static str,
    pub tool_analog: &'static str,
    pub description: &'static str,
    pub planner_node: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPanel {
    pub id: &'static str,
    pub product_analog: &'static str,
    pub data_shape: &'static str,
    pub visualization_families: Vec<&'static str>,
    pub refresh: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RendererContract {
    pub id: &'static str,
    pub analog: &'static str,
    pub output: &'static str,
    pub supports: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfServiceSurface {
    pub id: &'static str,
    pub analog: &'static str,
    pub audience: &'static str,
    pub capabilities: Vec<&'static str>,
}

pub fn platform_capabilities_payload() -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.platform-parity.v1",
        "parityMatrix": parity_matrix(),
        "semanticModels": semantic_models(),
        "connectors": connector_catalog(),
        "workbooks": workbook_blueprints(),
        "etl": etl_primitives(),
        "dashboardPanels": dashboard_panel_catalog(),
        "rendererContracts": renderer_contracts(),
        "selfService": self_service_surfaces(),
        "presentationTargets": presentation_targets()
    })
}

pub fn parity_matrix() -> Vec<ProductParity> {
    vec![
        ProductParity {
            product: "Tableau",
            category: "visual analytics",
            parity_goal: "expressive interactive dashboards and high-control visual encoding",
            implemented_surfaces: vec![
                "visualization genome",
                "2d/3d/4d/5d/xd channel encodings",
                "saved dashboard definitions",
                "presentation layer export",
            ],
            next_engine_work: vec![
                "interactive dashboard layout persistence",
                "pixel-level renderer testing",
                "published workbook permissions",
            ],
        },
        ProductParity {
            product: "Microsoft Power BI",
            category: "enterprise BI",
            parity_goal: "Power Query-style transforms plus DAX-like semantic measures",
            implemented_surfaces: vec![
                "semantic model catalog",
                "measure definitions",
                "ETL planner primitives",
            ],
            next_engine_work: vec![
                "DAX-compatible expression parser",
                "Power Query M import surface",
                "incremental refresh partitions",
            ],
        },
        ProductParity {
            product: "Qlik Sense",
            category: "associative analytics",
            parity_goal: "dynamic relationship discovery across all loaded fields",
            implemented_surfaces: vec![
                "dataset association graph endpoint",
                "categorical co-occurrence planner",
                "field cardinality profiles",
                "multi-dataset associative selection endpoint",
                "green/white/gray selection-state model",
            ],
            next_engine_work: vec![
                "relationship confidence scoring",
                "persisted selection sessions",
                "field alias and semantic relationship inference",
            ],
        },
        ProductParity {
            product: "Looker",
            category: "developer-first BI",
            parity_goal: "Git-friendly semantic layer with governed dimensions and measures",
            implemented_surfaces: vec![
                "LookML-inspired model descriptors",
                "route-derived API docs",
                "centralized metric definitions",
            ],
            next_engine_work: vec![
                "LookML parser",
                "model validation CLI",
                "warehouse SQL compilation targets",
            ],
        },
        ProductParity {
            product: "Sigma Computing",
            category: "spreadsheet over warehouse",
            parity_goal: "business-user grid interactions over live analytical results",
            implemented_surfaces: vec![
                "workbook blueprint",
                "cell formula planning",
                "live-query result grid contract",
            ],
            next_engine_work: vec![
                "columnar virtual sheet engine",
                "warehouse-backed lazy paging",
                "collaborative workbook state",
            ],
        },
        ProductParity {
            product: "Domo",
            category: "cloud BI and ETL",
            parity_goal: "broad connector catalog and visual ETL planning",
            implemented_surfaces: vec![
                "connector descriptor catalog",
                "Magic ETL-style primitives",
                "near-real-time ingest posture",
            ],
            next_engine_work: vec![
                "connector SDK",
                "streaming ingestion checkpoints",
                "mobile executive cards",
            ],
        },
        ProductParity {
            product: "Apache Superset",
            category: "open-source self-service BI",
            parity_goal: "SQL-first chart exploration with RBAC-aware publishing",
            implemented_surfaces: vec![
                "parser-backed SQL dialect endpoint",
                "self-service chart builder contract",
                "enforced RBAC policy surface",
            ],
            next_engine_work: vec![
                "role-backed chart ownership",
                "database connection registry",
                "SQL Lab history",
            ],
        },
        ProductParity {
            product: "Metabase",
            category: "lightweight self-service BI",
            parity_goal: "simple query builder and natural-language-friendly questions",
            implemented_surfaces: vec![
                "question builder contract",
                "saved dashboard catalog",
                "raw SQL fallback",
                "dataset profile summaries",
            ],
            next_engine_work: vec![
                "visual query builder execution",
                "saved questions",
                "natural language intent parser",
            ],
        },
        ProductParity {
            product: "Grafana",
            category: "time-series observability",
            parity_goal: "live panels, logs/metrics dialects, and alert-ready dashboard specs",
            implemented_surfaces: vec![
                "PromQL/LogQL frontends",
                "time-series panel catalog",
                "Prometheus metrics route",
                "alert rule catalog and evaluator",
            ],
            next_engine_work: vec![
                "alert notification channels",
                "Loki log frame adapter",
                "WebSocket live panel stream",
            ],
        },
        ProductParity {
            product: "D3.js",
            category: "programmatic rendering",
            parity_goal: "renderer-neutral specs with arbitrary channel control",
            implemented_surfaces: vec![
                "D3 renderer contract",
                "final-layer JSON",
                "evolution-generated encodings",
            ],
            next_engine_work: vec![
                "generated TypeScript renderer package",
                "SVG/canvas regression screenshots",
                "animation/tween timeline specs",
            ],
        },
        ProductParity {
            product: "Plotly / Dash",
            category: "scientific app visualization",
            parity_goal: "statistical and scientific dashboard specs for app embedding",
            implemented_surfaces: vec![
                "Plotly trace contract",
                "3d surface/volume candidates",
                "presentation layer export",
            ],
            next_engine_work: vec![
                "Dash callback blueprint export",
                "scientific chart catalog",
                "Python/R client bindings",
            ],
        },
        ProductParity {
            product: "Evidence.dev",
            category: "code-driven reporting",
            parity_goal: "Markdown plus SQL narrative reports checked into source control",
            implemented_surfaces: vec![
                "Evidence report blueprint",
                "Reveal markdown export",
                "SQL query examples",
            ],
            next_engine_work: vec![
                "Markdown report compiler",
                "repo build artifacts",
                "scheduled report publication",
            ],
        },
    ]
}

pub fn connector_catalog() -> Vec<ConnectorDescriptor> {
    vec![
        ConnectorDescriptor {
            id: "postgres",
            family: "warehouse",
            mode: "planned live SQL pushdown",
            auth: "service secret or workload identity",
            planner_notes: "compile LogicalPlan to PostgreSQL SQL for large fact tables",
        },
        ConnectorDescriptor {
            id: "bigquery",
            family: "warehouse",
            mode: "planned live SQL pushdown",
            auth: "service account workload identity",
            planner_notes: "Looker/Sigma-style cloud warehouse execution target",
        },
        ConnectorDescriptor {
            id: "snowflake",
            family: "warehouse",
            mode: "planned live SQL pushdown",
            auth: "key-pair or external OAuth",
            planner_notes: "large cloud warehouse target with lazy workbook paging",
        },
        ConnectorDescriptor {
            id: "csv-json",
            family: "file",
            mode: "implemented JSON records; planned CSV",
            auth: "operator upload",
            planner_notes: "local columnar ingestion path for self-service datasets",
        },
        ConnectorDescriptor {
            id: "prometheus-loki",
            family: "observability",
            mode: "planned query federation",
            auth: "cluster service auth",
            planner_notes: "Grafana parity path for metrics and log panels",
        },
        ConnectorDescriptor {
            id: "rest-webhook",
            family: "streaming",
            mode: "planned append/live diff ingestion",
            auth: "shared secret or signed webhook",
            planner_notes: "Domo-style near-real-time connector surface",
        },
    ]
}

pub fn semantic_models() -> Vec<SemanticModel> {
    vec![SemanticModel {
        id: "sales-analytics",
        lineage: "LookML/Power BI inspired semantic layer over logical datasets",
        dimensions: vec![
            DimensionDefinition {
                name: "region",
                kind: "category",
                sql: "${TABLE}.region",
            },
            DimensionDefinition {
                name: "segment",
                kind: "category",
                sql: "${TABLE}.segment",
            },
            DimensionDefinition {
                name: "event_date",
                kind: "time",
                sql: "${TABLE}.event_date",
            },
        ],
        measures: vec![
            MeasureDefinition {
                name: "total_revenue",
                aggregate: "sum",
                expression: "sum(revenue)",
                dax_analog: "SUM('sales'[revenue])",
            },
            MeasureDefinition {
                name: "avg_margin",
                aggregate: "avg",
                expression: "avg(margin)",
                dax_analog: "AVERAGE('sales'[margin])",
            },
            MeasureDefinition {
                name: "row_count",
                aggregate: "count",
                expression: "count(*)",
                dax_analog: "COUNTROWS('sales')",
            },
        ],
        calculations: vec![CalculationDefinition {
            name: "margin_band",
            expression_language: "dd-expression-v1",
            expression: "case when margin >= 0.30 then 'strong' when margin >= 0.20 then 'watch' else 'risk' end",
        }],
    }]
}

pub fn workbook_blueprints() -> Vec<WorkbookBlueprint> {
    vec![
        WorkbookBlueprint {
            id: "live-grid",
            persona: "Sigma-style business analyst",
            grid_model: "virtual sheet backed by LogicalPlan result pages",
            interactions: vec![
                "sort",
                "filter",
                "pivot",
                "formula column",
                "chart from selection",
            ],
            live_data_strategy:
                "result windows can refresh from a live connector without pre-aggregation",
        },
        WorkbookBlueprint {
            id: "executive-card-stack",
            persona: "Domo/Power BI mobile consumer",
            grid_model: "small metric cards bound to governed measures",
            interactions: vec!["drill", "annotate", "export", "subscribe"],
            live_data_strategy: "card refresh can use diff streams once live ingestion lands",
        },
    ]
}

pub fn etl_primitives() -> Vec<EtlPrimitive> {
    vec![
        EtlPrimitive {
            id: "select-columns",
            tool_analog: "Power Query / Magic ETL",
            description: "project a governed subset of fields",
            planner_node: "Projection",
        },
        EtlPrimitive {
            id: "filter-rows",
            tool_analog: "Power Query / Superset SQL Lab",
            description: "push simple predicates to the scan or connector",
            planner_node: "Filter",
        },
        EtlPrimitive {
            id: "group-aggregate",
            tool_analog: "Magic ETL / SQL query builder",
            description: "group rows and apply count/sum/avg/min/max",
            planner_node: "Aggregate",
        },
        EtlPrimitive {
            id: "formula-field",
            tool_analog: "DAX / Sigma formula column",
            description: "computed columns and measures over existing fields",
            planner_node: "CalculatedField",
        },
        EtlPrimitive {
            id: "union-join",
            tool_analog: "Magic ETL / warehouse model",
            description: "planned multi-dataset relation composition",
            planner_node: "JoinOrUnion",
        },
    ]
}

pub fn dashboard_panel_catalog() -> Vec<DashboardPanel> {
    vec![
        DashboardPanel {
            id: "time-series-line",
            product_analog: "Grafana",
            data_shape: "timestamp, metric, labels",
            visualization_families: vec!["line", "area", "stat", "alert threshold"],
            refresh: "poll now; WebSocket/live diff planned",
        },
        DashboardPanel {
            id: "business-kpi",
            product_analog: "Power BI / Domo",
            data_shape: "measure, comparison period, segment",
            visualization_families: vec!["card", "sparkline", "variance bar"],
            refresh: "governed measure refresh",
        },
        DashboardPanel {
            id: "exploration-chart",
            product_analog: "Tableau / Superset / Metabase",
            data_shape: "dimension, measure, optional facet",
            visualization_families: vec!["bar", "scatter", "histogram", "heatmap"],
            refresh: "query result cache planned",
        },
        DashboardPanel {
            id: "hyper-dimensional-scene",
            product_analog: "D3 / Plotly",
            data_shape: "n-dimensional encoded records",
            visualization_families: vec![
                "3d surface",
                "volume cloud",
                "parallel coordinates",
                "hyper-slice atlas",
            ],
            refresh: "renderer-neutral final layer",
        },
    ]
}

pub fn renderer_contracts() -> Vec<RendererContract> {
    vec![
        RendererContract {
            id: "d3-final-layer",
            analog: "D3.js",
            output: "JSON binding plan for SVG/canvas/WebGL renderers",
            supports: vec!["arbitrary channels", "transitions", "DOM/data joins"],
        },
        RendererContract {
            id: "plotly-traces",
            analog: "Plotly / Dash",
            output: "trace/layout/data frame blueprint",
            supports: vec!["scientific charts", "3d surfaces", "app callbacks"],
        },
        RendererContract {
            id: "evidence-markdown",
            analog: "Evidence.dev",
            output: "Markdown report with SQL and visualization placeholders",
            supports: vec!["narrative analytics", "repo review", "static publication"],
        },
        RendererContract {
            id: "office-openxml",
            analog: "PowerPoint / Google Slides",
            output: "presentation package layers and batch-update commands",
            supports: vec!["executive decks", "speaker notes", "slide renderer hints"],
        },
    ]
}

pub fn self_service_surfaces() -> Vec<SelfServiceSurface> {
    vec![
        SelfServiceSurface {
            id: "visual-query-builder",
            analog: "Metabase",
            audience: "non-technical product squads",
            capabilities: vec![
                "choose dataset",
                "pick dimensions",
                "pick measures",
                "chart suggestion",
            ],
        },
        SelfServiceSurface {
            id: "sql-lab",
            analog: "Apache Superset",
            audience: "analysts and data engineers",
            capabilities: vec!["raw SQL", "saved query plan", "chart from result"],
        },
        SelfServiceSurface {
            id: "semantic-explorer",
            analog: "Looker",
            audience: "governed BI developers",
            capabilities: vec!["modeled dimensions", "modeled measures", "metric lineage"],
        },
    ]
}

pub fn presentation_targets() -> Vec<&'static str> {
    vec![
        "powerpoint-openxml",
        "google-slides",
        "reveal-markdown",
        "final-layer-json",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_matrix_covers_named_competitors_and_libraries() {
        let names = parity_matrix()
            .into_iter()
            .map(|item| item.product)
            .collect::<Vec<_>>();

        for expected in [
            "Tableau",
            "Microsoft Power BI",
            "Qlik Sense",
            "Looker",
            "Sigma Computing",
            "Domo",
            "Apache Superset",
            "Metabase",
            "Grafana",
            "D3.js",
            "Plotly / Dash",
            "Evidence.dev",
        ] {
            assert!(names.contains(&expected));
        }
    }

    #[test]
    fn semantic_models_include_governed_measures() {
        let models = semantic_models();
        let measures = &models[0].measures;

        assert!(measures
            .iter()
            .any(|measure| measure.name == "total_revenue"));
        assert!(measures
            .iter()
            .any(|measure| !measure.dax_analog.is_empty()));
    }
}
