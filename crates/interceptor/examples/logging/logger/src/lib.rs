wit_bindgen::generate!({
    path: "../../wit",
    world: "logger-world",
    generate_all
});

use exports::modulewise::interceptor::advice::{
    Arg, Value, AfterAction, BeforeAction, Guest, GuestInvocation,
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
        Self { function_name, args }
    }

    fn before(&self) -> BeforeAction {
        log(Level::Info, "logging-advice", &format!("Before {} with Args: {:?}", self.function_name, self.args));
        BeforeAction::Proceed(self.args.clone())
    }

    fn after(&self, ret: Option<Value>) -> AfterAction {
        log(Level::Info, "logging-advice", &format!("After {} with Result: {:?}", self.function_name, ret));
        match ret {
            Some(Value::Str(s)) => AfterAction::Accept(Some(Value::Str(s.to_uppercase()))),
            Some(Value::NumS64(n)) => AfterAction::Accept(Some(Value::NumS64(n * n))),
            other => AfterAction::Accept(other),
        }
    }
}

export!(Logger);
