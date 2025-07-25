use crate::{children::TypedChildren, IntoView};
use futures::{channel::oneshot, future::join_all};
use hydration_context::{SerializedDataId, SharedContext};
use leptos_macro::component;
use or_poisoned::OrPoisoned;
use reactive_graph::{
    computed::ArcMemo,
    effect::RenderEffect,
    owner::{provide_context, ArcStoredValue, Owner},
    signal::ArcRwSignal,
    traits::{Get, Update, With, WithUntracked, WriteValue},
};
use rustc_hash::FxHashMap;
use std::{
    collections::VecDeque,
    fmt::Debug,
    mem,
    sync::{Arc, Mutex},
};
use tachys::{
    html::attribute::{any_attribute::AnyAttribute, Attribute},
    hydration::Cursor,
    reactive_graph::OwnedView,
    ssr::{StreamBuilder, StreamChunk},
    view::{
        add_attr::AddAnyAttr, Mountable, Position, PositionState, Render,
        RenderHtml,
    },
};
use throw_error::{Error, ErrorHook, ErrorId};

/// When you render a `Result<_, _>` in your view, in the `Err` case it will
/// render nothing, and search up through the view tree for an `<ErrorBoundary/>`.
/// This component lets you define a fallback that should be rendered in that
/// error case, allowing you to handle errors within a section of the interface.
///
/// ```
/// # use leptos::prelude::*;
/// #[component]
/// pub fn ErrorBoundaryExample() -> impl IntoView {
///   let (value, set_value) = signal(Ok(0));
///   let on_input =
///     move |ev| set_value.set(event_target_value(&ev).parse::<i32>());
///
///   view! {
///     <input type="text" on:input=on_input/>
///     <ErrorBoundary
///       fallback=move |_| view! { <p class="error">"Enter a valid number."</p>}
///     >
///       <p>"Value is: " {move || value.get()}</p>
///     </ErrorBoundary>
///   }
/// }
/// ```
///
/// ## Beginner's Tip: ErrorBoundary Requires Your Error To Implement std::error::Error.
/// `ErrorBoundary` requires your `Result<T,E>` to implement [IntoView](https://docs.rs/leptos/latest/leptos/trait.IntoView.html).
/// `Result<T,E>` only implements `IntoView` if `E` implements [std::error::Error](https://doc.rust-lang.org/std/error/trait.Error.html).
/// So, for instance, if you pass a `Result<T,String>` where `T` implements [IntoView](https://docs.rs/leptos/latest/leptos/trait.IntoView.html)
/// and attempt to render the error for the purposes of `ErrorBoundary` you'll get a compiler error like this.
///
/// ```rust,ignore
/// error[E0599]: the method `into_view` exists for enum `Result<ViewableLoginFlow, String>`, but its trait bounds were not satisfied
///    --> src/login.rs:229:32
///     |
/// 229 |                     err => err.into_view(),
///     |                                ^^^^^^^^^ method cannot be called on `Result<ViewableLoginFlow, String>` due to unsatisfied trait bounds
///     |
///     = note: the following trait bounds were not satisfied:
///             `<&Result<ViewableLoginFlow, std::string::String> as FnOnce<()>>::Output = _`
///             which is required by `&Result<ViewableLoginFlow, std::string::String>: leptos::IntoView`
///    ... more notes here ...
/// ```
///
/// For more information about how to easily implement `Error` see
/// [thiserror](https://docs.rs/thiserror/latest/thiserror/)
#[component]
pub fn ErrorBoundary<FalFn, Fal, Chil>(
    /// The elements that will be rendered, which may include one or more `Result<_>` types.
    children: TypedChildren<Chil>,
    /// A fallback that will be shown if an error occurs.
    fallback: FalFn,
) -> impl IntoView
where
    FalFn: FnMut(ArcRwSignal<Errors>) -> Fal + Send + 'static,
    Fal: IntoView + Send + 'static,
    Chil: IntoView + Send + 'static,
{
    let sc = Owner::current_shared_context();
    let boundary_id = sc.as_ref().map(|sc| sc.next_id()).unwrap_or_default();
    let initial_errors =
        sc.map(|sc| sc.errors(&boundary_id)).unwrap_or_default();

    let hook = Arc::new(ErrorBoundaryErrorHook::new(
        boundary_id.clone(),
        initial_errors,
    ));
    let errors = hook.errors.clone();
    let errors_empty = ArcMemo::new({
        let errors = errors.clone();
        move |_| errors.with(|map| map.is_empty())
    });
    let hook = hook as Arc<dyn ErrorHook>;

    let _guard = throw_error::set_error_hook(Arc::clone(&hook));
    let suspended_children = ErrorBoundarySuspendedChildren::default();

    let owner = Owner::new();
    let children = owner.with(|| {
        provide_context(Arc::clone(&hook));
        provide_context(suspended_children.clone());
        children.into_inner()()
    });

    OwnedView::new_with_owner(
        ErrorBoundaryView {
            hook,
            boundary_id,
            errors_empty,
            children,
            errors,
            fallback,
            suspended_children,
        },
        owner,
    )
}

pub(crate) type ErrorBoundarySuspendedChildren =
    ArcStoredValue<Vec<oneshot::Receiver<()>>>;

struct ErrorBoundaryView<Chil, FalFn> {
    hook: Arc<dyn ErrorHook>,
    boundary_id: SerializedDataId,
    errors_empty: ArcMemo<bool>,
    children: Chil,
    fallback: FalFn,
    errors: ArcRwSignal<Errors>,
    suspended_children: ErrorBoundarySuspendedChildren,
}

struct ErrorBoundaryViewState<Chil, Fal> {
    // the children are always present; we toggle between them and the fallback as needed
    children: Chil,
    fallback: Option<Fal>,
}

impl<Chil, Fal> Mountable for ErrorBoundaryViewState<Chil, Fal>
where
    Chil: Mountable,
    Fal: Mountable,
{
    fn unmount(&mut self) {
        if let Some(fallback) = &mut self.fallback {
            fallback.unmount();
        } else {
            self.children.unmount();
        }
    }

    fn mount(
        &mut self,
        parent: &tachys::renderer::types::Element,
        marker: Option<&tachys::renderer::types::Node>,
    ) {
        if let Some(fallback) = &mut self.fallback {
            fallback.mount(parent, marker);
        } else {
            self.children.mount(parent, marker);
        }
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        if let Some(fallback) = &self.fallback {
            fallback.insert_before_this(child)
        } else {
            self.children.insert_before_this(child)
        }
    }

    fn elements(&self) -> Vec<tachys::renderer::types::Element> {
        if let Some(fallback) = &self.fallback {
            fallback.elements()
        } else {
            self.children.elements()
        }
    }
}

impl<Chil, FalFn, Fal> Render for ErrorBoundaryView<Chil, FalFn>
where
    Chil: Render + 'static,
    FalFn: FnMut(ArcRwSignal<Errors>) -> Fal + Send + 'static,
    Fal: Render + 'static,
{
    type State = RenderEffect<ErrorBoundaryViewState<Chil::State, Fal::State>>;

    fn build(mut self) -> Self::State {
        let hook = Arc::clone(&self.hook);
        let _hook = throw_error::set_error_hook(Arc::clone(&hook));
        let mut children = Some(self.children.build());
        RenderEffect::new(
            move |prev: Option<
                ErrorBoundaryViewState<Chil::State, Fal::State>,
            >| {
                let _hook = throw_error::set_error_hook(Arc::clone(&hook));
                if let Some(mut state) = prev {
                    match (self.errors_empty.get(), &mut state.fallback) {
                        // no errors, and was showing fallback
                        (true, Some(fallback)) => {
                            fallback.insert_before_this(&mut state.children);
                            fallback.unmount();
                            state.fallback = None;
                        }
                        // yes errors, and was showing children
                        (false, None) => {
                            state.fallback = Some(
                                (self.fallback)(self.errors.clone()).build(),
                            );
                            state
                                .children
                                .insert_before_this(&mut state.fallback);
                            state.children.unmount();
                        }
                        // either there were no errors, and we were already showing the children
                        // or there are errors, but we were already showing the fallback
                        // in either case, rebuilding doesn't require us to do anything
                        _ => {}
                    }
                    state
                } else {
                    let fallback = (!self.errors_empty.get())
                        .then(|| (self.fallback)(self.errors.clone()).build());
                    ErrorBoundaryViewState {
                        children: children.take().unwrap(),
                        fallback,
                    }
                }
            },
        )
    }

    fn rebuild(self, state: &mut Self::State) {
        let new = self.build();
        let mut old = std::mem::replace(state, new);
        old.insert_before_this(state);
        old.unmount();
    }
}

impl<Chil, FalFn, Fal> AddAnyAttr for ErrorBoundaryView<Chil, FalFn>
where
    Chil: RenderHtml + 'static,
    FalFn: FnMut(ArcRwSignal<Errors>) -> Fal + Send + 'static,
    Fal: RenderHtml + Send + 'static,
{
    type Output<SomeNewAttr: Attribute> =
        ErrorBoundaryView<Chil::Output<SomeNewAttr::CloneableOwned>, FalFn>;

    fn add_any_attr<NewAttr: Attribute>(
        self,
        attr: NewAttr,
    ) -> Self::Output<NewAttr>
    where
        Self::Output<NewAttr>: RenderHtml,
    {
        let ErrorBoundaryView {
            hook,
            boundary_id,
            errors_empty,
            children,
            fallback,
            errors,
            suspended_children,
        } = self;
        ErrorBoundaryView {
            hook,
            boundary_id,
            errors_empty,
            children: children.add_any_attr(attr.into_cloneable_owned()),
            fallback,
            errors,
            suspended_children,
        }
    }
}

impl<Chil, FalFn, Fal> RenderHtml for ErrorBoundaryView<Chil, FalFn>
where
    Chil: RenderHtml + Send + 'static,
    FalFn: FnMut(ArcRwSignal<Errors>) -> Fal + Send + 'static,
    Fal: RenderHtml + Send + 'static,
{
    type AsyncOutput = ErrorBoundaryView<Chil::AsyncOutput, FalFn>;
    type Owned = Self;

    const MIN_LENGTH: usize = Chil::MIN_LENGTH;

    fn dry_resolve(&mut self) {
        self.children.dry_resolve();
    }

    async fn resolve(self) -> Self::AsyncOutput {
        let ErrorBoundaryView {
            hook,
            boundary_id,
            errors_empty,
            children,
            fallback,
            errors,
            suspended_children,
            ..
        } = self;
        ErrorBoundaryView {
            hook,
            boundary_id,
            errors_empty,
            children: children.resolve().await,
            fallback,
            errors,
            suspended_children,
        }
    }

    fn to_html_with_buf(
        mut self,
        buf: &mut String,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra_attrs: Vec<AnyAttribute>,
    ) {
        // first, attempt to serialize the children to HTML, then check for errors
        let _hook = throw_error::set_error_hook(self.hook);
        let mut new_buf = String::with_capacity(Chil::MIN_LENGTH);
        let mut new_pos = *position;
        self.children.to_html_with_buf(
            &mut new_buf,
            &mut new_pos,
            escape,
            mark_branches,
            extra_attrs.clone(),
        );

        // any thrown errors would've been caught here
        if self.errors.with_untracked(|map| map.is_empty()) {
            buf.push_str(&new_buf);
        } else {
            // otherwise, serialize the fallback instead
            (self.fallback)(self.errors).to_html_with_buf(
                buf,
                position,
                escape,
                mark_branches,
                extra_attrs,
            );
        }
    }

    fn to_html_async_with_buf<const OUT_OF_ORDER: bool>(
        mut self,
        buf: &mut StreamBuilder,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra_attrs: Vec<AnyAttribute>,
    ) where
        Self: Sized,
    {
        let _hook = throw_error::set_error_hook(Arc::clone(&self.hook));

        // first, attempt to serialize the children to HTML, then check for errors
        let mut new_buf = StreamBuilder::new(buf.clone_id());
        let mut new_pos = *position;
        self.children.to_html_async_with_buf::<OUT_OF_ORDER>(
            &mut new_buf,
            &mut new_pos,
            escape,
            mark_branches,
            extra_attrs.clone(),
        );

        let suspense_children =
            mem::take(&mut *self.suspended_children.write_value());

        // not waiting for any suspended children: just render
        if suspense_children.is_empty() {
            // any thrown errors would've been caught here
            if self.errors.with_untracked(|map| map.is_empty()) {
                buf.append(new_buf);
            } else {
                // otherwise, serialize the fallback instead
                let mut fallback = String::with_capacity(Fal::MIN_LENGTH);
                (self.fallback)(self.errors).to_html_with_buf(
                    &mut fallback,
                    position,
                    escape,
                    mark_branches,
                    extra_attrs,
                );
                buf.push_sync(&fallback);
            }
        } else {
            let mut position = *position;
            // if we're waiting for suspended children, we'll first wait for them to load
            // in this implementation, an ErrorBoundary that *contains* Suspense essentially acts
            // like a Suspense: it will wait for (all top-level) child Suspense to load before rendering anything
            let mut view_buf = StreamBuilder::new(new_buf.clone_id());
            view_buf.next_id();
            let hook = Arc::clone(&self.hook);
            view_buf.push_async(async move {
                let _hook = throw_error::set_error_hook(Arc::clone(&hook));
                let _ = join_all(suspense_children).await;

                let mut my_chunks = VecDeque::new();
                for chunk in new_buf.take_chunks() {
                    match chunk {
                        StreamChunk::Sync(data) => {
                            my_chunks.push_back(StreamChunk::Sync(data))
                        }
                        StreamChunk::Async { chunks } => {
                            let chunks = chunks.await;
                            my_chunks.extend(chunks);
                        }
                        StreamChunk::OutOfOrder { chunks } => {
                            let chunks = chunks.await;
                            my_chunks.push_back(StreamChunk::OutOfOrder {
                                chunks: Box::pin(async move { chunks }),
                            });
                        }
                    }
                }

                if self.errors.with_untracked(|map| map.is_empty()) {
                    // if no errors, just go ahead with the stream
                    my_chunks
                } else {
                    // otherwise, serialize the fallback instead
                    let mut fallback = String::with_capacity(Fal::MIN_LENGTH);
                    (self.fallback)(self.errors).to_html_with_buf(
                        &mut fallback,
                        &mut position,
                        escape,
                        mark_branches,
                        extra_attrs,
                    );
                    my_chunks.clear();
                    my_chunks.push_back(StreamChunk::Sync(fallback));
                    my_chunks
                }
            });
            buf.append(view_buf);
        }
    }

    fn hydrate<const FROM_SERVER: bool>(
        mut self,
        cursor: &Cursor,
        position: &PositionState,
    ) -> Self::State {
        let mut children = Some(self.children);
        let hook = Arc::clone(&self.hook);
        let cursor = cursor.to_owned();
        let position = position.to_owned();
        RenderEffect::new(
            move |prev: Option<
                ErrorBoundaryViewState<Chil::State, Fal::State>,
            >| {
                let _hook = throw_error::set_error_hook(Arc::clone(&hook));
                if let Some(mut state) = prev {
                    match (self.errors_empty.get(), &mut state.fallback) {
                        // no errors, and was showing fallback
                        (true, Some(fallback)) => {
                            fallback.insert_before_this(&mut state.children);
                            state.fallback.unmount();
                            state.fallback = None;
                        }
                        // yes errors, and was showing children
                        (false, None) => {
                            state.fallback = Some(
                                (self.fallback)(self.errors.clone()).build(),
                            );
                            state
                                .children
                                .insert_before_this(&mut state.fallback);
                            state.children.unmount();
                        }
                        // either there were no errors, and we were already showing the children
                        // or there are errors, but we were already showing the fallback
                        // in either case, rebuilding doesn't require us to do anything
                        _ => {}
                    }
                    state
                } else {
                    let children = children.take().unwrap();
                    let (children, fallback) = if self.errors_empty.get() {
                        (
                            children.hydrate::<FROM_SERVER>(&cursor, &position),
                            None,
                        )
                    } else {
                        (
                            children.build(),
                            Some(
                                (self.fallback)(self.errors.clone())
                                    .hydrate::<FROM_SERVER>(&cursor, &position),
                            ),
                        )
                    };

                    ErrorBoundaryViewState { children, fallback }
                }
            },
        )
    }

    async fn hydrate_async(
        self,
        cursor: &Cursor,
        position: &PositionState,
    ) -> Self::State {
        let mut children = Some(self.children);
        let hook = Arc::clone(&self.hook);
        let cursor = cursor.to_owned();
        let position = position.to_owned();

        let fallback_fn = Arc::new(Mutex::new(self.fallback));
        let initial = {
            let errors_empty = self.errors_empty.clone();
            let errors = self.errors.clone();
            let fallback_fn = Arc::clone(&fallback_fn);
            async move {
                let children = children.take().unwrap();
                let (children, fallback) = if errors_empty.get() {
                    (children.hydrate_async(&cursor, &position).await, None)
                } else {
                    let children = children.build();
                    let fallback =
                        (fallback_fn.lock().or_poisoned())(errors.clone());
                    let fallback =
                        fallback.hydrate_async(&cursor, &position).await;
                    (children, Some(fallback))
                };

                ErrorBoundaryViewState { children, fallback }
            }
        };

        RenderEffect::new_with_async_value(
            move |prev: Option<
                ErrorBoundaryViewState<Chil::State, Fal::State>,
            >| {
                let _hook = throw_error::set_error_hook(Arc::clone(&hook));
                if let Some(mut state) = prev {
                    match (self.errors_empty.get(), &mut state.fallback) {
                        // no errors, and was showing fallback
                        (true, Some(fallback)) => {
                            fallback.insert_before_this(&mut state.children);
                            state.fallback.unmount();
                            state.fallback = None;
                        }
                        // yes errors, and was showing children
                        (false, None) => {
                            state.fallback = Some(
                                (fallback_fn.lock().or_poisoned())(
                                    self.errors.clone(),
                                )
                                .build(),
                            );
                            state
                                .children
                                .insert_before_this(&mut state.fallback);
                            state.children.unmount();
                        }
                        // either there were no errors, and we were already showing the children
                        // or there are errors, but we were already showing the fallback
                        // in either case, rebuilding doesn't require us to do anything
                        _ => {}
                    }
                    state
                } else {
                    unreachable!()
                }
            },
            initial,
        )
        .await
    }

    fn into_owned(self) -> Self::Owned {
        self
    }
}

#[derive(Debug)]
struct ErrorBoundaryErrorHook {
    errors: ArcRwSignal<Errors>,
    id: SerializedDataId,
    shared_context: Option<Arc<dyn SharedContext + Send + Sync>>,
}

impl ErrorBoundaryErrorHook {
    pub fn new(
        id: SerializedDataId,
        initial_errors: impl IntoIterator<Item = (ErrorId, Error)>,
    ) -> Self {
        Self {
            errors: ArcRwSignal::new(Errors(
                initial_errors.into_iter().collect(),
            )),
            id,
            shared_context: Owner::current_shared_context(),
        }
    }
}

impl ErrorHook for ErrorBoundaryErrorHook {
    fn throw(&self, error: Error) -> ErrorId {
        // generate a unique ID
        let key: ErrorId = Owner::current_shared_context()
            .map(|sc| sc.next_id())
            .unwrap_or_default()
            .into();

        // register it with the shared context, so that it can be serialized from server to client
        // as needed
        if let Some(sc) = &self.shared_context {
            sc.register_error(self.id.clone(), key.clone(), error.clone());
        }

        // add it to the reactive map of errors
        self.errors.update(|map| {
            map.insert(key.clone(), error);
        });

        // return the key, which will be owned by the Result being rendered and can be used to
        // unregister this error if it is rebuilt
        key
    }

    fn clear(&self, id: &throw_error::ErrorId) {
        self.errors.update(|map| {
            map.remove(id);
        });
    }
}

/// A struct to hold all the possible errors that could be provided by child Views
#[derive(Debug, Clone, Default)]
#[repr(transparent)]
pub struct Errors(FxHashMap<ErrorId, Error>);

impl Errors {
    /// Returns `true` if there are no errors.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Add an error to Errors that will be processed by `<ErrorBoundary/>`
    pub fn insert<E>(&mut self, key: ErrorId, error: E)
    where
        E: Into<Error>,
    {
        self.0.insert(key, error.into());
    }

    /// Add an error with the default key for errors outside the reactive system
    pub fn insert_with_default_key<E>(&mut self, error: E)
    where
        E: Into<Error>,
    {
        self.0.insert(Default::default(), error.into());
    }

    /// Remove an error to Errors that will be processed by `<ErrorBoundary/>`
    pub fn remove(&mut self, key: &ErrorId) -> Option<Error> {
        self.0.remove(key)
    }

    /// An iterator over all the errors, in arbitrary order.
    #[inline(always)]
    pub fn iter(&self) -> Iter<'_> {
        Iter(self.0.iter())
    }
}

impl IntoIterator for Errors {
    type Item = (ErrorId, Error);
    type IntoIter = IntoIter;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter(self.0.into_iter())
    }
}

/// An owning iterator over all the errors contained in the [`Errors`] struct.
#[repr(transparent)]
pub struct IntoIter(std::collections::hash_map::IntoIter<ErrorId, Error>);

impl Iterator for IntoIter {
    type Item = (ErrorId, Error);

    #[inline(always)]
    fn next(
        &mut self,
    ) -> std::option::Option<<Self as std::iter::Iterator>::Item> {
        self.0.next()
    }
}

/// An iterator over all the errors contained in the [`Errors`] struct.
#[repr(transparent)]
pub struct Iter<'a>(std::collections::hash_map::Iter<'a, ErrorId, Error>);

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a ErrorId, &'a Error);

    #[inline(always)]
    fn next(
        &mut self,
    ) -> std::option::Option<<Self as std::iter::Iterator>::Item> {
        self.0.next()
    }
}
