// Copyright 2025 The kmesh Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
use http::HeaderValue;
use parking_lot::Mutex;

use crate::{request_id::RequestId, trace_info::TraceInfo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceParent {
    /// When the full context of the parent is available.
    Full(TraceInfo),
    /// For a trace started from a root node, only the Trace ID is avaiable...
    TraceId(Option<u128>),
}

#[derive(Debug)]
pub struct TraceContext {
    parent: TraceParent,
    child: Mutex<Option<TraceInfo>>,
    request_id: Option<RequestId>,
    client_trace_id: Option<HeaderValue>,
    should_sample: bool,
}

impl TraceContext {
    /// Creates a new (root) `TraceContext` instance.
    pub fn new(trace_id: Option<u128>) -> Self {
        Self {
            parent: TraceParent::TraceId(trace_id),
            child: Mutex::new(None),
            request_id: None,
            client_trace_id: None,
            should_sample: false,
        }
    }

    pub fn with_parent(self, parent: TraceInfo) -> Self {
        let should_sample = parent.sampled();
        Self { parent: TraceParent::Full(parent), should_sample, ..self }
    }

    pub fn with_child(self, child: TraceInfo) -> Self {
        let should_sample = child.sampled(); // sampled of parent and child is consistent
        Self { child: Mutex::new(Some(child)), should_sample, ..self }
    }

    pub fn with_request_id(self, request_id: Option<RequestId>) -> Self {
        Self { request_id, ..self }
    }

    pub fn with_client_trace_id(self, client_trace_id: Option<HeaderValue>) -> Self {
        Self { client_trace_id, ..self }
    }

    pub fn with_should_sample(self, should_sample: bool) -> Self {
        Self { should_sample, ..self }
    }

    #[inline]
    pub fn parent(&self) -> Option<&TraceInfo> {
        match self.parent {
            TraceParent::Full(ref trace_info) => Some(trace_info),
            TraceParent::TraceId(_) => None,
        }
    }

    #[inline]
    pub fn spawn_child(&self) {
        let mut guard = self.child.lock();
        *guard = match &self.parent {
            TraceParent::Full(trace_info) => Some(trace_info.clone().into_child()),
            TraceParent::TraceId(trace_id) => {
                Some(TraceInfo::new(true, crate::trace_info::TraceProvider::W3CTraceContext, *trace_id).into_child())
            },
        };
    }

    #[inline]
    pub fn map_child<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&TraceInfo) -> R,
    {
        self.child.lock().as_ref().map(f)
    }

    #[inline]
    pub fn request_id(&self) -> Option<&HeaderValue> {
        self.request_id.as_ref().map(|id| id.as_ref())
    }

    #[inline]
    pub fn client_trace_id(&self) -> Option<&HeaderValue> {
        self.client_trace_id.as_ref()
    }

    #[inline]
    pub fn is_root_node(&self) -> bool {
        matches!(self.parent, TraceParent::TraceId(_))
    }

    #[inline]
    pub fn should_sample(&self) -> bool {
        self.should_sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_info::{TraceInfo, TraceProvider};
    use http::HeaderValue;

    fn create_sample_trace_info() -> TraceInfo {
        TraceInfo::new(true, TraceProvider::W3CTraceContext, Some(0x1234567890abcdef))
    }

    fn create_sample_request_id() -> RequestId {
        RequestId::Propagate(HeaderValue::from_static("test-request-id"))
    }

    #[test]
    fn test_new_with_trace_id() {
        let trace_id = Some(0x1234567890abcdef);
        let context = TraceContext::new(trace_id);

        assert_eq!(context.parent, TraceParent::TraceId(trace_id));
        assert!(context.child.lock().is_none());
        assert!(context.request_id.is_none());
        assert!(context.client_trace_id.is_none());
        assert!(!context.should_sample);
        assert!(context.is_root_node());
    }

    #[test]
    fn test_new_without_trace_id() {
        let context = TraceContext::new(None);

        assert_eq!(context.parent, TraceParent::TraceId(None));
        assert!(context.child.lock().is_none());
        assert!(context.request_id.is_none());
        assert!(context.client_trace_id.is_none());
        assert!(!context.should_sample);
        assert!(context.is_root_node());
    }

    #[test]
    fn test_with_parent() {
        let trace_info = create_sample_trace_info();
        let sampled = trace_info.sampled();
        let context = TraceContext::new(None).with_parent(trace_info.clone());

        assert_eq!(context.parent, TraceParent::Full(trace_info));
        assert_eq!(context.should_sample, sampled);
        assert!(!context.is_root_node());
    }

    #[test]
    fn test_with_child() {
        let child_info = create_sample_trace_info();
        let sampled = child_info.sampled();
        let context = TraceContext::new(None).with_child(child_info.clone());

        assert_eq!(*context.child.lock(), Some(child_info));
        assert_eq!(context.should_sample, sampled);
    }

    #[test]
    fn test_with_request_id() {
        let request_id = Some(create_sample_request_id());
        let context = TraceContext::new(None).with_request_id(request_id.clone());

        assert_eq!(context.request_id, request_id);
    }

    #[test]
    fn test_with_request_id_none() {
        let context = TraceContext::new(None).with_request_id(None);

        assert!(context.request_id.is_none());
    }

    #[test]
    fn test_with_client_trace_id() {
        let client_trace_id = Some(HeaderValue::from_static("client-trace-123"));
        let context = TraceContext::new(None).with_client_trace_id(client_trace_id.clone());

        assert_eq!(context.client_trace_id, client_trace_id);
    }

    #[test]
    fn test_with_client_trace_id_none() {
        let context = TraceContext::new(None).with_client_trace_id(None);

        assert!(context.client_trace_id.is_none());
    }

    #[test]
    fn test_with_should_sample() {
        let context = TraceContext::new(None).with_should_sample(true);

        assert!(context.should_sample);

        let context = TraceContext::new(None).with_should_sample(false);

        assert!(!context.should_sample);
    }

    #[test]
    fn test_spawn_child_with_trace_id_parent() {
        let trace_id = Some(0x1234567890abcdef);
        let context = TraceContext::new(trace_id);

        assert!(context.child.lock().is_none());

        context.spawn_child();

        assert!(context.child.lock().is_some());
        let guard = context.child.lock();
        let child = guard.as_ref().unwrap();
        assert_eq!(child.trace_id(), trace_id.unwrap());
        assert!(child.sampled());
        assert_eq!(child.provider(), TraceProvider::W3CTraceContext);
    }

    #[test]
    fn test_spawn_child_with_full_parent() {
        let parent_info = create_sample_trace_info();
        let context = TraceContext::new(None).with_parent(parent_info.clone());
        assert!(context.child.lock().is_none());
        context.spawn_child();
        assert!(context.child.lock().is_some());

        let guard = context.child.lock();
        let child = guard.as_ref().unwrap();
        assert_eq!(child.trace_id(), parent_info.trace_id());
        // Child should inherit from parent
        assert_ne!(child.span_id(), parent_info.span_id());
    }

    #[test]
    fn test_parent_getter() {
        // Test with TraceId parent (should return None)
        let context = TraceContext::new(Some(0x123));
        assert!(context.parent().is_none());

        // Test with Full parent (should return Some)
        let trace_info = create_sample_trace_info();
        let context = TraceContext::new(None).with_parent(trace_info.clone());
        assert_eq!(context.parent(), Some(&trace_info));
    }

    #[test]
    fn test_child_getter() {
        let context = TraceContext::new(None);
        assert!(context.map_child(|_| ()).is_none());

        let child_info = create_sample_trace_info();
        let context = TraceContext::new(None).with_child(child_info.clone());
        assert_eq!(context.map_child(|child| child.clone()), Some(child_info));
    }

    #[test]
    fn test_request_id_getter() {
        let context = TraceContext::new(None);
        assert!(context.request_id().is_none());

        let request_id = create_sample_request_id();
        let context = TraceContext::new(None).with_request_id(Some(request_id.clone()));
        assert_eq!(context.request_id(), Some(request_id.as_ref()));
    }

    #[test]
    fn test_client_trace_id_getter() {
        let context = TraceContext::new(None);
        assert!(context.client_trace_id().is_none());

        let client_trace_id = HeaderValue::from_static("client-trace-456");
        let context = TraceContext::new(None).with_client_trace_id(Some(client_trace_id.clone()));
        assert_eq!(context.client_trace_id(), Some(&client_trace_id));
    }

    #[test]
    fn test_is_root_node() {
        // Root node with trace ID
        let context = TraceContext::new(Some(0x123));
        assert!(context.is_root_node());

        // Root node without trace ID
        let context = TraceContext::new(None);
        assert!(context.is_root_node());

        // Non-root node with parent
        let trace_info = create_sample_trace_info();
        let context = TraceContext::new(None).with_parent(trace_info);
        assert!(!context.is_root_node());
    }

    #[test]
    fn test_should_sample_getter() {
        let context = TraceContext::new(None);
        assert!(!context.should_sample());

        let context = TraceContext::new(None).with_should_sample(true);
        assert!(context.should_sample());
    }

    #[test]
    fn test_builder_pattern_chaining() {
        let trace_info = create_sample_trace_info();
        let request_id = create_sample_request_id();
        let client_trace_id = HeaderValue::from_static("client-trace-789");

        let context = TraceContext::new(Some(0x123))
            .with_parent(trace_info.clone())
            .with_request_id(Some(request_id.clone()))
            .with_client_trace_id(Some(client_trace_id.clone()))
            .with_should_sample(true);

        assert_eq!(context.parent, TraceParent::Full(trace_info.clone()));
        assert_eq!(context.request_id, Some(request_id));
        assert_eq!(context.client_trace_id, Some(client_trace_id));
        assert!(context.should_sample());
        assert!(!context.is_root_node());
    }

    #[test]
    fn test_trace_parent_equality() {
        let trace_id = Some(0x123);
        let parent1 = TraceParent::TraceId(trace_id);
        let parent2 = TraceParent::TraceId(trace_id);
        assert_eq!(parent1, parent2);

        let trace_info1 = create_sample_trace_info();
        let trace_info2 = create_sample_trace_info();
        let parent3 = TraceParent::Full(trace_info1);
        let parent4 = TraceParent::Full(trace_info2);
        assert_eq!(parent3, parent4);
    }

    #[test]
    fn test_multiple_spawn_child_calls() {
        let trace_id = Some(0x789);
        let context = TraceContext::new(trace_id);

        // First spawn
        context.spawn_child();
        let first_child = context.child.lock().clone();
        assert!(first_child.is_some());

        // Second spawn (should overwrite the previous child)
        context.spawn_child();
        let second_child = context.child.lock().clone();
        assert!(second_child.is_some());

        // Children should be different (different span IDs)
        assert_ne!(first_child, second_child);
    }
}
