wit_bindgen::generate!({
    path: "../wit",
    world: "logger-world",
    generate_all
});

use exports::modulewise::interceptor::advice::{
    AfterAction, Arg, BeforeAction, Guest, GuestInvocation, Value,
};
use wasi::logging::logging::{Level, log};

struct Logger;

impl Guest for Logger {
    type Invocation = LoggingInvocation;
}

struct LoggingInvocation {
    function_name: String,
    args: Vec<Arg>,
}

impl GuestInvocation for LoggingInvocation {
    fn new(function_name: String, args: Vec<Arg>) -> Self {
        Self {
            function_name,
            args,
        }
    }

    fn before(&self) -> BeforeAction {
        let args: Vec<_> = self
            .args
            .iter()
            .map(|a| format!("{}: {}", a.name, fmt_value(&a.value)))
            .collect();
        log(
            Level::Info,
            "interceptor",
            &format!("Before {}({})", self.function_name, args.join(", ")),
        );
        BeforeAction::Proceed(self.args.clone())
    }

    fn after(&self, ret: Option<Value>) -> AfterAction {
        let result = ret.as_ref().map(fmt_value).unwrap_or("void".into());
        log(
            Level::Info,
            "interceptor",
            &format!("After {} -> {result}", self.function_name),
        );
        AfterAction::Accept(ret)
    }
}

fn fmt_value(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("\"{s}\""),
        Value::NumS64(n) => n.to_string(),
        Value::NumU64(n) => n.to_string(),
        Value::NumF32(n) => n.to_string(),
        Value::NumF64(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Complex(t) => format!("<{t}>"),
    }
}

export!(Logger);
