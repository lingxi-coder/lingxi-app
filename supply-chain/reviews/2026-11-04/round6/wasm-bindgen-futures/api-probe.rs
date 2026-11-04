#![allow(dead_code)]
use std::future::Future;
use wasm_bindgen_futures::{future_to_promise, future_to_promise_typed, spawn_local, JsFuture};
use wasm_bindgen_futures::wasm_bindgen::{self, JsValue};
use wasm_bindgen_futures::js_sys;

fn future_type_identity(promise: js_sys::Promise) -> (JsFuture, js_sys::futures::JsFuture) {
    let via_shim: js_sys::futures::JsFuture = JsFuture::from(promise.clone());
    let direct: JsFuture = js_sys::futures::JsFuture::from(promise);
    (direct, via_shim)
}
fn spawn<F: Future<Output = ()> + 'static>(future: F) {
    spawn_local(future);
}
fn promise<F: Future<Output = Result<JsValue, JsValue>> + 'static>(future: F) -> js_sys::Promise {
    future_to_promise(future)
}
fn typed<T, F>(future: F) -> js_sys::Promise<<T as wasm_bindgen::sys::Promising>::Resolution>
where
    F: Future<Output = Result<T, JsValue>> + 'static,
    T: wasm_bindgen::sys::Promising + wasm_bindgen::convert::FromWasmAbi + wasm_bindgen::JsGeneric,
    <T as wasm_bindgen::sys::Promising>::Resolution: wasm_bindgen::JsGeneric,
{
    future_to_promise_typed(future)
}
#[cfg(feature = "futures-core-03-stream")]
fn stream_type_identity(value: wasm_bindgen_futures::stream::JsStream) -> js_sys::futures::stream::JsStream {
    value
}
fn main() {}
