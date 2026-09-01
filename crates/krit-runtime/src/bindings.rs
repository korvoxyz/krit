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
        },
        imports: { default: trappable },
    });
}
