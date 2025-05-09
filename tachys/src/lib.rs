//! Allows rendering user interfaces based on a statically-typed view tree.
//!
//! This view tree is generic over rendering backends, and agnostic about reactivity/change
//! detection.

// this is specifically used for `unsized_const_params` below
// this allows us to use const generic &'static str for static text nodes and attributes
#![allow(incomplete_features)]
#![cfg_attr(
    all(feature = "nightly", rustc_nightly),
    feature(unsized_const_params)
)]
#![deny(missing_docs)]

/// Commonly-used traits.
pub mod prelude {
    pub use crate::{
        html::{
            attribute::{
                any_attribute::IntoAnyAttribute,
                aria::AriaAttributes,
                custom::CustomAttribute,
                global::{
                    ClassAttribute, GlobalAttributes, GlobalOnAttributes,
                    OnAttribute, OnTargetAttribute, PropAttribute,
                    StyleAttribute,
                },
                IntoAttributeValue,
            },
            directive::DirectiveAttribute,
            element::{ElementChild, ElementExt, InnerHtmlAttribute},
            node_ref::NodeRefAttribute,
        },
        renderer::{dom::Dom, Renderer},
        view::{
            add_attr::AddAnyAttr,
            any_view::{AnyView, IntoAny, IntoMaybeErased},
            IntoRender, Mountable, Render, RenderHtml,
        },
    };
}

use wasm_bindgen::JsValue;
use web_sys::Node;

/// Helpers for interacting with the DOM.
pub mod dom;
/// Types for building a statically-typed HTML view tree.
pub mod html;
/// Supports adding interactivity to HTML.
pub mod hydration;
/// Types for MathML.
pub mod mathml;
/// Defines various backends that can render views.
pub mod renderer;
/// Rendering views to HTML.
pub mod ssr;
/// Types for SVG.
pub mod svg;
/// Core logic for manipulating views.
pub mod view;

pub use either_of as either;
#[cfg(feature = "islands")]
#[doc(hidden)]
pub use wasm_bindgen;
#[cfg(feature = "islands")]
#[doc(hidden)]
pub use web_sys;

/// View implementations for the `oco_ref` crate (cheaply-cloned string types).
#[cfg(feature = "oco")]
pub mod oco;
/// View implementations for the `reactive_graph` crate.
#[cfg(feature = "reactive_graph")]
pub mod reactive_graph;

/// A type-erased container.
pub mod erased;

pub(crate) trait UnwrapOrDebug {
    type Output;

    fn or_debug(self, el: &Node, label: &'static str);

    fn ok_or_debug(
        self,
        el: &Node,
        label: &'static str,
    ) -> Option<Self::Output>;
}

impl<T> UnwrapOrDebug for Result<T, JsValue> {
    type Output = T;

    #[track_caller]
    fn or_debug(self, el: &Node, name: &'static str) {
        #[cfg(any(debug_assertions, leptos_debuginfo))]
        {
            if let Err(err) = self {
                let location = std::panic::Location::caller();
                web_sys::console::warn_3(
                    &JsValue::from_str(&format!(
                        "[WARNING] Non-fatal error at {location}, while \
                         calling {name} on "
                    )),
                    el,
                    &err,
                );
            }
        }
        #[cfg(not(any(debug_assertions, leptos_debuginfo)))]
        {
            _ = self;
        }
    }

    #[track_caller]
    fn ok_or_debug(
        self,
        el: &Node,
        name: &'static str,
    ) -> Option<Self::Output> {
        #[cfg(any(debug_assertions, leptos_debuginfo))]
        {
            if let Err(err) = &self {
                let location = std::panic::Location::caller();
                web_sys::console::warn_3(
                    &JsValue::from_str(&format!(
                        "[WARNING] Non-fatal error at {location}, while \
                         calling {name} on "
                    )),
                    el,
                    err,
                );
            }
            self.ok()
        }
        #[cfg(not(any(debug_assertions, leptos_debuginfo)))]
        {
            self.ok()
        }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! or_debug {
    ($action:expr, $el:expr, $label:literal) => {
        if cfg!(any(debug_assertions, leptos_debuginfo)) {
            $crate::UnwrapOrDebug::or_debug($action, $el, $label);
        } else {
            _ = $action;
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! ok_or_debug {
    ($action:expr, $el:expr, $label:literal) => {
        if cfg!(any(debug_assertions, leptos_debuginfo)) {
            $crate::UnwrapOrDebug::ok_or_debug($action, $el, $label)
        } else {
            $action.ok()
        }
    };
}
