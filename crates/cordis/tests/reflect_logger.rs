use cordis::{
    Accessor, Context, ExporterConfig, Inject, LogArg, LoggerIntercept, LoggerLevel, PluginOutput,
    Result, Value, default_format, plugin_sync,
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

/// Upstream parity (logger.spec name resolution): an explicit name wins over
/// the intercept name, which wins over the fiber-derived name, which falls
/// back to "root".
#[test]
fn logger_name_resolution_precedence_chain() -> Result<()> {
    let root = Context::new();

    // Outside any plugin the fiber-derived name falls back to "root".
    assert_eq!(root.logger().name, "root");
    // An explicit name beats everything.
    assert_eq!(root.named_logger("explicit").name, "explicit");

    // An intercept name beats the fiber-derived name...
    let intercepted = root.intercept(
        "logger",
        LoggerIntercept {
            name: Some("intercepted".to_owned()),
            level: None,
        },
    );
    assert_eq!(intercepted.logger().name, "intercepted");
    // ...but an explicit name still beats the intercept.
    assert_eq!(intercepted.named_logger("explicit").name, "explicit");

    // Inside a named plugin the default logger uses the hyphenated fiber
    // name, and the intercept wins there too when present.
    let names = Arc::new(Mutex::new(Vec::new()));
    let plugin = plugin_sync::<(), _>("MyDriver", Inject::none(), {
        let names = names.clone();
        move |ctx, _| {
            names.lock().unwrap().push(ctx.logger().name.clone());
            names
                .lock()
                .unwrap()
                .push(ctx.named_logger("mine").name.clone());
            Ok(PluginOutput::none())
        }
    });
    let fiber = intercepted.plugin_default(plugin);
    fiber.try_wait()?;
    assert_eq!(*names.lock().unwrap(), vec!["intercepted", "mine"]);
    Ok(())
}
