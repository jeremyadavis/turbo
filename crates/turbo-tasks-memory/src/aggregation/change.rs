use std::hash::Hash;

use super::{AggegatingNode, AggregationContext, AggregationNode, PreparedOperation, StackVec};

impl<I: Clone + Eq + Hash, D> AggregationNode<I, D> {
    #[must_use]
    pub fn apply_change<C: AggregationContext<NodeRef = I, Data = D>>(
        &mut self,
        ctx: &C,
        change: C::DataChange,
    ) -> Option<PreparedChange<C>> {
        match self {
            AggregationNode::Leaf { uppers, .. } => (!uppers.is_empty()).then(|| PreparedChange {
                uppers: uppers.iter().cloned().collect::<StackVec<_>>(),
                change,
            }),
            AggregationNode::Aggegating(aggegating) => {
                let AggegatingNode { data, uppers, .. } = &mut **aggegating;
                let change = ctx.apply_change(data, &change);
                if uppers.is_empty() {
                    None
                } else if let Some(change) = change {
                    Some(PreparedChange {
                        uppers: uppers.iter().cloned().collect::<StackVec<_>>(),
                        change,
                    })
                } else {
                    None
                }
            }
        }
    }

    pub fn apply_change_ref<'l, C: AggregationContext<NodeRef = I, Data = D>>(
        &mut self,
        ctx: &C,
        change: &'l C::DataChange,
    ) -> Option<PreparedChangeRef<'l, C>> {
        match self {
            AggregationNode::Leaf { uppers, .. } => {
                (!uppers.is_empty()).then(|| PreparedChangeRef::Borrowed {
                    uppers: uppers.iter().cloned().collect::<StackVec<_>>(),
                    change,
                })
            }
            AggregationNode::Aggegating(aggegating) => {
                let AggegatingNode { data, uppers, .. } = &mut **aggegating;
                let change = ctx.apply_change(data, change);
                if uppers.is_empty() {
                    None
                } else if let Some(change) = change {
                    Some(PreparedChangeRef::Owned {
                        uppers: uppers.iter().cloned().collect::<StackVec<_>>(),
                        change,
                    })
                } else {
                    None
                }
            }
        }
    }
}

pub struct PreparedChange<C: AggregationContext> {
    uppers: StackVec<C::NodeRef>,
    change: C::DataChange,
}

impl<C: AggregationContext> PreparedOperation<C> for PreparedChange<C> {
    type Result = ();
    fn apply(self, ctx: &C) {
        let prepared = self
            .uppers
            .into_iter()
            .filter_map(|upper_id| ctx.node(&upper_id).apply_change_ref(ctx, &self.change))
            .collect::<StackVec<_>>();
        prepared.apply(ctx);
    }
}

pub enum PreparedChangeRef<'l, C: AggregationContext> {
    Borrowed {
        uppers: StackVec<C::NodeRef>,
        change: &'l C::DataChange,
    },
    Owned {
        uppers: StackVec<C::NodeRef>,
        change: C::DataChange,
    },
}

impl<'l, C: AggregationContext> PreparedOperation<C> for PreparedChangeRef<'l, C> {
    type Result = ();
    fn apply(self, ctx: &C) {
        let (uppers, change) = match self {
            PreparedChangeRef::Borrowed { uppers, change } => (uppers, change),
            PreparedChangeRef::Owned { uppers, ref change } => (uppers, change),
        };
        let prepared = uppers
            .into_iter()
            .filter_map(|upper_id| ctx.node(&upper_id).apply_change_ref(ctx, change))
            .collect::<StackVec<_>>();
        prepared.apply(ctx);
    }
}

pub fn apply_change<C: AggregationContext>(ctx: &C, mut node: C::Guard<'_>, change: C::DataChange) {
    let p = node.apply_change(ctx, change);
    drop(node);
    p.apply(ctx);
}

pub fn apply_change_ref<C: AggregationContext>(
    ctx: &C,
    mut node: C::Guard<'_>,
    change: &C::DataChange,
) {
    let p = node.apply_change_ref(ctx, change);
    drop(node);
    p.apply(ctx);
}
