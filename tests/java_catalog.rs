//! The Java (JNI) bindgen backend: the shapes×language grid rendered for Java.
//! A JNI backend is unique — it emits TWO artifacts, so the gates cover both:
//! the Rust JNI GLUE (`java_binding`, `#[no_mangle] extern "system"` fns routing
//! to `crate::core_impl`) and the Java SOURCE classes (`java_sources`, the
//! `.java` files with `native` declarations + `System.loadLibrary`).
//!
//! Like the other backends this is a token-level string test inside `cargo test`
//! (no JDK / javac is invoked — the generated Java is proven by shape, and the
//! generated Rust is proven parseable because `java_binding` runs it through
//! rustfmt, which rejects malformed input).
//!
//! straitjacket-allow-file:duplication — this suite is DELIBERATELY parallel to
//! `tests/php_catalog.rs`'s per-language assertions (same fixture load, same
//! name-only-enum parity setup); the cross-language token parity is the point.

use fluessig::api::{ApiDoc, ApiInterface, ApiOp, ApiType, Shape};

const API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/api.json"
));

const M0: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/atilla_m0.api.json"
));

/// atilla's M0 surface, hand-built: the stateless `Atilla` interface with one
/// readonly unary `version(): string` (fallible in M0).
fn m0_api() -> ApiDoc {
    ApiDoc {
        fluessig: fluessig::ir::Versions {
            format: 1,
            emitter: Some("0.0.0".into()),
            compiler: Some("1.14.0".into()),
        },
        source: Some("atilla.tsp".into()),
        models: Vec::new(),
        unions: Vec::new(),
        consts: Vec::new(),
        interfaces: vec![ApiInterface {
            name: "Atilla".into(),
            doc: None,
            single_threaded: false,
            ops: vec![ApiOp {
                name: "version".into(),
                doc: Some("The atilla engine version.".into()),
                shape: Shape::Unary,
                is_async: false,
                infallible: false,
                readonly: true,
                destructive: false,
                worker: false,
                stream_error: None,
                result_error: None,
                params: Vec::new(),
                returns: ApiType::Scalar("string".into()),
                bindings: Default::default(),
            }],
        }],
    }
}

#[test]
fn java_m0_glue_routes_through_the_core_trait() {
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let glue = fluessig::bindgen::java_binding(&m0_api(), &enums, None);

    // the JNI prelude + the RuntimeException throw seam
    for needle in [
        "use jni::JNIEnv;",
        "use jni::objects::{JByteArray, JClass, JObject, JString, JValue};",
        "use fluessig_runtime::{Poll, PollStream};",
        "fn throw(env: &mut JNIEnv, e: impl std::fmt::Display)",
        "env.throw_new(\"java/lang/RuntimeException\", e.to_string())",
    ] {
        assert!(glue.contains(needle), "M0 glue missing {needle:?}:\n{glue}");
    }

    // the version op is a JNI extern fn named by its JVM mangling, routed through
    // the generated core trait + `crate::core_impl::AtillaImpl` (the house-style
    // core split, not a direct `atilla_core::version()` call).
    assert!(
        glue.contains("pub extern \"system\" fn Java_fluessig_Atilla_version<'local>"),
        "JNI extern fn for version:\n{glue}"
    );
    assert!(
        glue.contains("<crate::core_impl::AtillaImpl as AtillaCore>::version()"),
        "routes through the core trait:\n{glue}"
    );
    assert!(
        glue.contains("pub trait AtillaCore"),
        "emits the core trait:\n{glue}"
    );
    // fallible op → the throw seam; returns a jstring.
    assert!(
        glue.contains("-> jstring") && glue.contains("throw(env, __e);"),
        "fallible version throws on Err:\n{glue}"
    );
    // no ctor → no init/handle for a stateless op.
    assert!(
        !glue.contains("Java_fluessig_Atilla_init"),
        "no ctor for a stateless op:\n{glue}"
    );

    // the fixture on disk generates identically to the hand-built ApiDoc.
    let from_file = fluessig::api::load_api(M0).expect("m0 fixture loads");
    assert_eq!(
        fluessig::bindgen::java_binding(&from_file, &enums, None),
        glue,
        "fixture and hand-built ApiDoc generate identically"
    );
}

#[test]
fn java_m0_source_is_a_static_native_class() {
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let sources = fluessig::bindgen::java_sources(&m0_api(), &enums);
    let (path, src) = sources
        .iter()
        .find(|(p, _)| p == "fluessig/Atilla.java")
        .expect("Atilla.java emitted");
    assert_eq!(path, "fluessig/Atilla.java");

    for needle in [
        "package fluessig;",
        "public final class Atilla {",
        "static { System.loadLibrary(\"fluessig\"); }",
        "public static native String version();",
        // a stateless class reads as a static namespace: a private ctor.
        "private Atilla() {}",
    ] {
        assert!(
            src.contains(needle),
            "Atilla.java missing {needle:?}:\n{src}"
        );
    }
}

#[test]
fn java_renders_the_union_fixture() {
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = Vec::new();
    let glue = fluessig::bindgen::java_binding(&api, &enums, None);
    let sources = fluessig::bindgen::java_sources(&api, &enums);
    let src = |name: &str| -> String {
        sources
            .iter()
            .find(|(p, _)| p == name)
            .unwrap_or_else(|| panic!("{name} not emitted"))
            .1
            .clone()
    };

    // ── DTOs: a plain Rust struct on the glue side, a class + getters in Java ──
    assert!(
        glue.contains("pub struct AgentMessage"),
        "DTO Rust struct:\n{glue}"
    );
    assert!(
        glue.contains(
            "fn AgentMessage_to_j<'a>(env: &mut JNIEnv<'a>, v: AgentMessage) -> JObject<'a>"
        ),
        "DTO to-Java marshaller:\n{glue}"
    );
    assert!(
        glue.contains(
            "fn AgentMessage_from_j<'a>(env: &mut JNIEnv<'a>, o: &JObject<'a>) -> AgentMessage"
        ),
        "DTO from-Java marshaller:\n{glue}"
    );
    let dto = src("fluessig/AgentMessage.java");
    assert!(
        dto.contains("public final class AgentMessage"),
        "DTO class:\n{dto}"
    );
    assert!(
        dto.contains("public String getRole()"),
        "DTO getter:\n{dto}"
    );

    // ── union: rides as its JSON envelope String (the shared carrier) + a class ──
    assert!(
        glue.contains("pub payload: String"),
        "union field is the envelope String carrier:\n{glue}"
    );
    let union = src("fluessig/EventPayload.java");
    assert!(
        union.contains("public final class EventPayload") && union.contains("getPayload()"),
        "union envelope class:\n{union}"
    );

    // ── the stateful `Watch` interface: ctor `open` → a native handle-pointer ──
    assert!(
        glue.contains("pub extern \"system\" fn Java_fluessig_Watch_init<'local>"),
        "ctor init fn:\n{glue}"
    );
    assert!(
        glue.contains("Box::into_raw(Box::new(Arc::new(__c))) as jlong"),
        "init leaks a Box<Arc<Impl>> as the handle:\n{glue}"
    );
    assert!(
        glue.contains("pub extern \"system\" fn Java_fluessig_Watch_free<'local>")
            && glue.contains("*mut Arc<crate::core_impl::WatchImpl>"),
        "free reclaims the handle:\n{glue}"
    );
    let watch = src("fluessig/Watch.java");
    assert!(
        watch.contains("public Watch(String path) { this.handle = init(path); }"),
        "Java ctor threads the native handle:\n{watch}"
    );
    assert!(
        watch.contains("private static native long init(String path);")
            && watch.contains("public void close()"),
        "Java handle surface:\n{watch}"
    );

    // ── stream: a poll cursor over Box<dyn PollStream<Event>> + an Events class ──
    assert!(
        glue.contains("pub extern \"system\" fn Java_fluessig_Events_poll<'local>"),
        "stream cursor poll fn:\n{glue}"
    );
    assert!(
        glue.contains("&*(cursor as *const Box<dyn PollStream<Event>>)"),
        "cursor is a Box<dyn PollStream<Event>>:\n{glue}"
    );
    for needle in [
        "Poll::Item(__v) => return",
        "Poll::Idle => continue,",
        "Poll::Closed => return std::ptr::null_mut(),",
        "Poll::Failed(__e) => {",
    ] {
        assert!(
            glue.contains(needle),
            "cursor poll arm missing {needle:?}:\n{glue}"
        );
    }
    let events = src("fluessig/Events.java");
    assert!(
        events.contains("public final class Events")
            && events.contains("public Optional<Event> next()")
            && events.contains("private static native Object poll(long cursor);"),
        "Events poll-cursor class:\n{events}"
    );
    assert!(
        watch.contains("public Events events(Long afterIdx) { return new Events(nativeEvents(this.handle, afterIdx)); }"),
        "Watch opens the stream cursor:\n{watch}"
    );

    // ── async ops (emit/clear/run are `#[fluessig(async)]`) → CompletableFuture ──
    assert!(
        watch.contains("import java.util.concurrent.CompletableFuture;"),
        "async ops pull in CompletableFuture:\n{watch}"
    );
    assert!(
        watch.contains("public CompletableFuture<Long> clear() {")
            && watch.contains("CompletableFuture.supplyAsync(() -> nativeClear(this.handle))"),
        "async unary → a CompletableFuture wrapper over the blocking native:\n{watch}"
    );
    // the underlying native stays private + blocking.
    assert!(
        watch.contains("private static native long nativeClear(long handle);"),
        "the async op's native method is private/blocking:\n{watch}"
    );

    // the runtime contract is imported, not redeclared inline.
    assert!(
        glue.contains("use fluessig_runtime::{Poll, PollStream};"),
        "shared streaming contract imported from fluessig-runtime:\n{glue}"
    );
}

#[test]
fn java_name_only_enums_render_as_plain_rust_plus_a_java_enum() {
    // A name-only vocabulary lowers to a plain Rust enum with parse()/wire() over
    // its snake_case wire tokens (Java sees them as strings), PLUS a standalone
    // Java `enum` class carrying those wire tokens with fromWire/toWire.
    let api = fluessig::api::load_api(API).unwrap();
    let enums: Vec<fluessig::bindgen::EnumDesc> = vec![(
        "CapabilityKind".to_string(),
        vec![
            fluessig::bindgen::EnumVariant::plain("dispatch"),
            fluessig::bindgen::EnumVariant::plain("isolation_vm"),
        ],
    )];
    let glue = fluessig::bindgen::java_binding(&api, &enums, None);
    assert!(
        glue.contains("pub enum CapabilityKind"),
        "Rust enum type:\n{glue}"
    );
    assert!(
        glue.contains("\"isolation_vm\" => Ok(Self::IsolationVm)"),
        "parse token parity:\n{glue}"
    );
    assert!(
        glue.contains("Self::IsolationVm => \"isolation_vm\""),
        "wire token parity:\n{glue}"
    );

    let sources = fluessig::bindgen::java_sources(&api, &enums);
    let (_, java_enum) = sources
        .iter()
        .find(|(p, _)| p == "fluessig/CapabilityKind.java")
        .expect("CapabilityKind.java emitted");
    assert!(
        java_enum.contains("public enum CapabilityKind"),
        "Java enum class:\n{java_enum}"
    );
    assert!(
        java_enum.contains("IsolationVm(\"isolation_vm\")"),
        "Java enum carries the wire token:\n{java_enum}"
    );
    assert!(
        java_enum.contains("public static CapabilityKind fromWire(String w)"),
        "Java enum fromWire seam:\n{java_enum}"
    );
}
