fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy = model_runtime::settings::SettingsPolicy::default();
    println!("{}", serde_json::to_string_pretty(&policy.schema())?);
    Ok(())
}
