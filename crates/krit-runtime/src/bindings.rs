pub(crate) mod pure {
    wasmtime::component::bindgen!({
        world: "pure-program",
        path: "../../wit/runtime.wit",
    });
}

pub(crate) mod stdout {
    wasmtime::component::bindgen!({
        world: "program",
        path: "../../wit/runtime.wit",
        imports: {
            "krit:runtime/stdout.write-int": trappable,
            "krit:runtime/stdout.write-bool": trappable,
            "krit:runtime/stdout.write-unit": trappable,
        },
    });
}

pub(crate) mod webhook {
    wasmtime::component::bindgen!({
        world: "webhook-host-program",
        path: "../../wit/runtime.wit",
        with: {
            "krit:runtime/secrets.secret": crate::SecretHandle,
            "krit:runtime/database.transaction": crate::database::TransactionHandle,
        },
        imports: { default: trappable },
    });
}

pub(crate) mod job {
    wasmtime::component::bindgen!({
        world: "job-host-program",
        path: "../../wit/runtime.wit",
        with: {
            "krit:runtime/stdout": crate::bindings::webhook::krit::runtime::stdout,
            "krit:runtime/config": crate::bindings::webhook::krit::runtime::config,
            "krit:runtime/secrets": crate::bindings::webhook::krit::runtime::secrets,
            "krit:runtime/http": crate::bindings::webhook::krit::runtime::http,
            "krit:runtime/http-anonymous":
                crate::bindings::webhook::krit::runtime::http_anonymous,
            "krit:runtime/ai": crate::bindings::webhook::krit::runtime::ai,
            "krit:runtime/logging": crate::bindings::webhook::krit::runtime::logging,
            "krit:runtime/state": crate::bindings::webhook::krit::runtime::state,
            "krit:runtime/queue": crate::bindings::webhook::krit::runtime::queue,
            "krit:runtime/objects-read": crate::bindings::webhook::krit::runtime::objects_read,
            "krit:runtime/objects-write": crate::bindings::webhook::krit::runtime::objects_write,
            "krit:runtime/database": crate::bindings::webhook::krit::runtime::database,
        },
        imports: { default: trappable },
    });
}

pub(crate) mod schedule {
    wasmtime::component::bindgen!({
        world: "schedule-host-program",
        path: "../../wit/runtime.wit",
        with: {
            "krit:runtime/stdout": crate::bindings::webhook::krit::runtime::stdout,
            "krit:runtime/config": crate::bindings::webhook::krit::runtime::config,
            "krit:runtime/secrets": crate::bindings::webhook::krit::runtime::secrets,
            "krit:runtime/http": crate::bindings::webhook::krit::runtime::http,
            "krit:runtime/http-anonymous":
                crate::bindings::webhook::krit::runtime::http_anonymous,
            "krit:runtime/ai": crate::bindings::webhook::krit::runtime::ai,
            "krit:runtime/logging": crate::bindings::webhook::krit::runtime::logging,
            "krit:runtime/state": crate::bindings::webhook::krit::runtime::state,
            "krit:runtime/queue": crate::bindings::webhook::krit::runtime::queue,
            "krit:runtime/objects-read": crate::bindings::webhook::krit::runtime::objects_read,
            "krit:runtime/objects-write": crate::bindings::webhook::krit::runtime::objects_write,
            "krit:runtime/database": crate::bindings::webhook::krit::runtime::database,
        },
        imports: { default: trappable },
    });
}
