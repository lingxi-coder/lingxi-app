fn one_stream() { let _ = futures_util::stream_select!(futures_util::stream::empty::<()>()); }
fn duplicate_default() { let _ = async { futures_util::select_biased! { default => 1, default => 2 } }; }
#[futures_macro::test_internal(extra)]
fn invalid_attribute_argument() {}
#[futures_macro::test_internal]
fn invalid_sync_test() {}
