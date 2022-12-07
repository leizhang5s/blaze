// Copyright 2022 The Blaze Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::pin::Pin;
use std::task::{Context, Poll, ready};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::Result;
use datafusion::physical_plan::{RecordBatchStream, SendableRecordBatchStream};
use datafusion::physical_plan::coalesce_batches::concat_batches;
use datafusion::physical_plan::metrics::BaselineMetrics;
use futures::{Stream, StreamExt};

pub struct CoalesceStream {
    input: SendableRecordBatchStream,
    staging_batches: Vec<RecordBatch>,
    staging_rows: usize,
    batch_size: usize,
    metrics: BaselineMetrics,
}

impl CoalesceStream {
    pub fn new(
        input: SendableRecordBatchStream,
        batch_size: usize,
        metrics: BaselineMetrics,
    ) -> Self {
        Self {
            input,
            staging_batches: vec![],
            staging_rows: 0,
            batch_size,
            metrics,
        }
    }

    fn coalesce(&mut self) -> Result<RecordBatch> {
        let coalesced = concat_batches(
            &self.schema(),
            &std::mem::take(&mut self.staging_batches),
            self.staging_rows,
        )?;
        self.staging_rows = 0;
        Ok(coalesced)
    }
}

impl RecordBatchStream for CoalesceStream {
    fn schema(&self) -> SchemaRef {
        self.input.schema()
    }
}

impl Stream for CoalesceStream {
    type Item = datafusion::arrow::error::Result<RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.input.poll_next_unpin(cx)).transpose()? {
                Some(batch) => {
                    if batch.num_rows() > 0 {
                        self.staging_rows += batch.num_rows();
                        self.staging_batches.push(batch);
                        if self.staging_rows >= self.batch_size {
                            let coalesced = self.coalesce()?;
                            return self.metrics.record_poll(
                                Poll::Ready(Some(Ok(coalesced)))
                            );
                        }
                        continue;
                    }
                }
                None if !self.staging_batches.is_empty() => {
                    let coalesced = self.coalesce()?;
                    return self.metrics.record_poll(
                        Poll::Ready(Some(Ok(coalesced)))
                    );
                }
                None => {
                    return self.metrics.record_poll(
                        Poll::Ready(None)
                    );
                }
            }
        }
    }
}