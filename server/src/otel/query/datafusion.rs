//! DataFusion analytics queries on Parquet files

use datafusion::arrow::array as arrow_array;
use datafusion::datasource::listing::{ListingOptions, ListingTableUrl};
use datafusion::execution::context::SessionContext;
use std::path::PathBuf;
use std::sync::Arc;

use crate::otel::error::OtelError;

/// DataFusion query executor for analytics on Parquet files
pub struct DataFusionExecutor {
    ctx: SessionContext,
}

impl DataFusionExecutor {
    /// Create a new DataFusion executor
    pub async fn new(traces_dir: PathBuf) -> Result<Self, OtelError> {
        let ctx = SessionContext::new();

        // Register the spans table from Parquet files
        let parquet_path = traces_dir.join("**/*.parquet");
        let path_str = parquet_path.to_string_lossy();

        // Only register if files exist
        if traces_dir.exists() {
            let listing_options = ListingOptions::new(Arc::new(
                datafusion::datasource::file_format::parquet::ParquetFormat::default(),
            ))
            .with_file_extension(".parquet");

            let table_path = ListingTableUrl::parse(&path_str).map_err(OtelError::Query)?;

            // This will fail silently if no files exist yet, which is fine
            let _ = ctx
                .register_listing_table("spans", table_path.as_str(), listing_options, None, None)
                .await;
        }

        Ok(Self { ctx })
    }

    /// Execute a SQL query against the Parquet files
    pub async fn query(
        &self,
        sql: &str,
    ) -> Result<Vec<datafusion::arrow::array::RecordBatch>, OtelError> {
        let df = self.ctx.sql(sql).await.map_err(OtelError::Query)?;

        let batches = df.collect().await.map_err(OtelError::Query)?;

        Ok(batches)
    }

    /// Get token usage summary by model
    pub async fn token_usage_by_model(&self) -> Result<Vec<ModelTokenUsage>, OtelError> {
        let sql = r#"
            SELECT
                gen_ai_request_model as model,
                COUNT(*) as span_count,
                SUM(usage_input_tokens) as total_input_tokens,
                SUM(usage_output_tokens) as total_output_tokens,
                SUM(usage_total_tokens) as total_tokens
            FROM spans
            WHERE gen_ai_request_model IS NOT NULL
            GROUP BY gen_ai_request_model
            ORDER BY total_tokens DESC
        "#;

        let batches = self.query(sql).await?;

        let mut results = Vec::new();
        for batch in batches {
            let models = batch.column(0).as_any().downcast_ref::<arrow_array::StringArray>();
            let counts = batch.column(1).as_any().downcast_ref::<arrow_array::Int64Array>();
            let inputs = batch.column(2).as_any().downcast_ref::<arrow_array::Int64Array>();
            let outputs = batch.column(3).as_any().downcast_ref::<arrow_array::Int64Array>();
            let totals = batch.column(4).as_any().downcast_ref::<arrow_array::Int64Array>();

            if let (Some(models), Some(counts), Some(inputs), Some(outputs), Some(totals)) =
                (models, counts, inputs, outputs, totals)
            {
                for i in 0..batch.num_rows() {
                    if let Some(model) = models.value(i).into() {
                        results.push(ModelTokenUsage {
                            model: model.to_string(),
                            span_count: counts.value(i),
                            total_input_tokens: inputs.value(i),
                            total_output_tokens: outputs.value(i),
                            total_tokens: totals.value(i),
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get span duration percentiles by framework
    pub async fn duration_stats_by_framework(
        &self,
    ) -> Result<Vec<FrameworkDurationStats>, OtelError> {
        let sql = r#"
            SELECT
                detected_framework,
                COUNT(*) as span_count,
                AVG(duration_ns) as avg_duration_ns,
                MIN(duration_ns) as min_duration_ns,
                MAX(duration_ns) as max_duration_ns
            FROM spans
            WHERE duration_ns IS NOT NULL
            GROUP BY detected_framework
            ORDER BY span_count DESC
        "#;

        let batches = self.query(sql).await?;

        let mut results = Vec::new();
        for batch in batches {
            let frameworks = batch.column(0).as_any().downcast_ref::<arrow_array::StringArray>();
            let counts = batch.column(1).as_any().downcast_ref::<arrow_array::Int64Array>();
            let avgs = batch.column(2).as_any().downcast_ref::<arrow_array::Float64Array>();
            let mins = batch.column(3).as_any().downcast_ref::<arrow_array::Int64Array>();
            let maxs = batch.column(4).as_any().downcast_ref::<arrow_array::Int64Array>();

            if let (Some(frameworks), Some(counts), Some(avgs), Some(mins), Some(maxs)) =
                (frameworks, counts, avgs, mins, maxs)
            {
                for i in 0..batch.num_rows() {
                    if let Some(framework) = frameworks.value(i).into() {
                        results.push(FrameworkDurationStats {
                            framework: framework.to_string(),
                            span_count: counts.value(i),
                            avg_duration_ns: avgs.value(i) as i64,
                            min_duration_ns: mins.value(i),
                            max_duration_ns: maxs.value(i),
                        });
                    }
                }
            }
        }

        Ok(results)
    }
}

/// Token usage by model
#[derive(Debug, Clone)]
pub struct ModelTokenUsage {
    pub model: String,
    pub span_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
}

/// Duration statistics by framework
#[derive(Debug, Clone)]
pub struct FrameworkDurationStats {
    pub framework: String,
    pub span_count: i64,
    pub avg_duration_ns: i64,
    pub min_duration_ns: i64,
    pub max_duration_ns: i64,
}
