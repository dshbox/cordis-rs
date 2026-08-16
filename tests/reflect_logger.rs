use cordis::{
    Accessor, Context, ExporterConfig, LogArg, LoggerIntercept, LoggerLevel, Result, Value,
    default_format,
};
use std::sync::{Arc, Mutex};

#[test]
fn accessor_alias_and_write_ownership() -> Result<()> {
    let root = Context::new();
    let stored = Arc::new(Mutex::new(10_u32));
    let read = stored.clone();
    let write = stored.clone();
    let _accessor = root.accessor(
        "answer",
        Accessor::read_write(
            move |_| Ok(Some(Value::new(*read.lock().unwrap()))),
            move |_, value| {
                *write.lock().unwrap() = *value.downcast::<u32>()?;
                Ok(())
            },
        ),
    )?;
    assert_eq!(*root.require::<u32>("answer")?, 10);
    root.set("answer", 42_u32)?;
    assert_eq!(*root.require::<u32>("answer")?, 42);

    let _service = root.provide("primary", String::from("ok"))?;
    let _alias = root.reflect().alias("secondary", "primary")?;
    assert_eq!(root.require::<String>("secondary")?.as_str(), "ok");
    Ok(())
}

#[test]
fn logger_buffers_exports_formats_and_intercepts() -> Result<()> {
    let root = Context::new();
    root.logger_service().set_buffer_size(2);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let mut config = ExporterConfig::default();
    config
        .levels
        .insert("default".to_owned(), LoggerLevel::Debug);
    let exporter = root
        .logger_service()
        .exporter_fn(config.clone(), move |message| {
            sink.lock().unwrap().push(message.clone());
        })?;

    root.logger()
        .info("%s #%d", [LogArg::from("hello"), LogArg::from(3)]);
    root.logger().debug("debug", []);
    root.logger().info("second", []);
    assert_eq!(root.logger_service().buffer().len(), 2);
    assert_eq!(captured.lock().unwrap().len(), 3);
    assert_eq!(
        default_format(&config, &captured.lock().unwrap()[0]),
        "hello #3"
    );

    let intercepted = root.intercept(
        "logger",
        LoggerIntercept {
            name: Some("custom".to_owned()),
            level: Some(LoggerLevel::Debug),
        },
    );
    intercepted.logger().debug("named", []);
    assert_eq!(captured.lock().unwrap().last().unwrap().name, "custom");

    exporter.dispose()?;
    root.logger().info("not exported", []);
    assert_eq!(captured.lock().unwrap().len(), 4);
    Ok(())
}
