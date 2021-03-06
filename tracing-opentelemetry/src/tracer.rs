use opentelemetry::api::trace as otel;
use opentelemetry::sdk::trace::{SamplingDecision, Tracer};

/// An interface for authors of OpenTelemetry SDKs to build pre-sampled tracers.
///
/// The OpenTelemetry spec does not allow trace ids to be updated after a span
/// has been created. In order to associate extracted parent trace ids with
/// existing `tracing` spans, `tracing-opentelemetry` builds up otel span data
/// using a [`SpanBuilder`] instead, and creates / exports full otel spans only
/// when the associated `tracing` span is closed. However, in order to properly
/// inject otel [`SpanReference`] information to downstream requests, the sampling
/// state must now be known _before_ the otel span has been created.
///
/// The logic for coming to a sampling decision and creating an injectable span
/// context from a [`SpanBuilder`] is encapsulated in the
/// [`PreSampledTracer::sampled_span_reference`] method and has been implemented
/// for the standard OpenTelemetry SDK, but this trait may be implemented by
/// authors of alternate OpenTelemetry SDK implementations if they wish to have
/// `tracing` compatibility.
///
/// See the [`OpenTelemetrySpanExt::set_parent`] and
/// [`OpenTelemetrySpanExt::context`] methods for example usage.
///
/// [`Tracer`]: https://docs.rs/opentelemetry/latest/opentelemetry/api/trace/tracer/trait.Tracer.html
/// [`SpanBuilder`]: https://docs.rs/opentelemetry/latest/opentelemetry/api/trace/tracer/struct.SpanBuilder.html
/// [`SpanReference`]: https://docs.rs/opentelemetry/latest/opentelemetry/api/trace/span_reference/struct.SpanReference.html
/// [`PreSampledTracer::sampled_span_reference`]: trait.PreSampledTracer.html#tymethod.sampled_span_reference
/// [`OpenTelemetrySpanExt::set_parent`]: trait.OpenTelemetrySpanExt.html#tymethod.set_parent
/// [`OpenTelemetrySpanExt::context`]: trait.OpenTelemetrySpanExt.html#tymethod.context
pub trait PreSampledTracer {
    /// Produce a pre-sampled span reference for the given span builder.
    fn sampled_span_reference(&self, builder: &mut otel::SpanBuilder) -> otel::SpanReference;

    /// Generate a new trace id.
    fn new_trace_id(&self) -> otel::TraceId;

    /// Generate a new span id.
    fn new_span_id(&self) -> otel::SpanId;
}

impl PreSampledTracer for otel::NoopTracer {
    fn sampled_span_reference(&self, builder: &mut otel::SpanBuilder) -> otel::SpanReference {
        builder
            .parent_reference
            .clone()
            .unwrap_or_else(otel::SpanReference::empty_context)
    }

    fn new_trace_id(&self) -> otel::TraceId {
        otel::TraceId::invalid()
    }

    fn new_span_id(&self) -> otel::SpanId {
        otel::SpanId::invalid()
    }
}

impl PreSampledTracer for Tracer {
    fn sampled_span_reference(&self, builder: &mut otel::SpanBuilder) -> otel::SpanReference {
        let span_id = builder.span_id.unwrap_or_else(|| {
            self.provider()
                .map(|provider| provider.config().id_generator.new_span_id())
                .unwrap_or_else(otel::SpanId::invalid)
        });
        let (trace_id, trace_flags) = builder
            .parent_reference
            .as_ref()
            .filter(|parent_reference| parent_reference.is_valid())
            .map(|parent_reference| (parent_reference.trace_id(), parent_reference.trace_flags()))
            .unwrap_or_else(|| {
                let trace_id = builder.trace_id.unwrap_or_else(|| {
                    self.provider()
                        .map(|provider| provider.config().id_generator.new_trace_id())
                        .unwrap_or_else(otel::TraceId::invalid)
                });

                // ensure sampling decision is recorded so all span references have consistent flags
                let sampling_decision = if let Some(result) = builder.sampling_result.as_ref() {
                    result.decision.clone()
                } else if let Some(provider) = self.provider().as_ref() {
                    let mut result = provider.config().default_sampler.should_sample(
                        builder.parent_reference.as_ref(),
                        trace_id,
                        &builder.name,
                        builder
                            .span_kind
                            .as_ref()
                            .unwrap_or(&otel::SpanKind::Internal),
                        builder.attributes.as_ref().unwrap_or(&Vec::new()),
                        builder.links.as_ref().unwrap_or(&Vec::new()),
                    );

                    // Record additional attributes resulting from sampling
                    if let Some(attributes) = &mut builder.attributes {
                        attributes.append(&mut result.attributes)
                    } else {
                        builder.attributes = Some(result.attributes);
                    }

                    result.decision
                } else {
                    SamplingDecision::Drop
                };

                let trace_flags = if sampling_decision == SamplingDecision::RecordAndSample {
                    otel::TRACE_FLAG_SAMPLED
                } else {
                    0
                };

                (trace_id, trace_flags)
            });

        otel::SpanReference::new(trace_id, span_id, trace_flags, false, Default::default())
    }

    fn new_trace_id(&self) -> otel::TraceId {
        self.provider()
            .map(|provider| provider.config().id_generator.new_trace_id())
            .unwrap_or_else(otel::TraceId::invalid)
    }

    fn new_span_id(&self) -> otel::SpanId {
        self.provider()
            .map(|provider| provider.config().id_generator.new_span_id())
            .unwrap_or_else(otel::SpanId::invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::api::trace::{SpanBuilder, TracerProvider};
    use opentelemetry::sdk;

    #[test]
    fn assigns_default_ids_if_missing() {
        let provider = sdk::trace::TracerProvider::default();
        let tracer = provider.get_tracer("test", None);
        let mut builder = SpanBuilder::from_name("empty".to_string());
        builder.trace_id = None;
        builder.span_id = None;
        let span_reference = tracer.sampled_span_reference(&mut builder);

        assert!(span_reference.is_valid());
    }
}
