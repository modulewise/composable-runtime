#![no_main]

wit_bindgen::generate!({
    path: "../wit",
    world: "logger-world",
    generate_all
});

use exports::modulewise::interceptor::advice::{
    AfterAction, Arg, BeforeAction, Guest, GuestInvocation, Value,
};
use wasi::logging::logging::{log, Level};

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
        log(
            Level::Info,
            "interceptor",
            &format!("Before {} with args: {:?}", self.function_name, self.args),
        );
        BeforeAction::Proceed(self.args.clone())
    }

    fn after(&self, ret: Option<Value>) -> AfterAction {
        log(
            Level::Info,
            "interceptor",
            &format!("After {} with result: {:?}", self.function_name, ret),
        );
        AfterAction::Accept(ret)
    }
}

export!(Logger);
