use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase", description = "Unly Agent Commands")]
pub enum Command {
    #[command(description = "Start or reset the conversation.")]
    Start,
    #[command(description = "Show this help message.")]
    Help,
    #[command(description = "Show current status and health.")]
    Status,
    #[command(description = "List available models.")]
    Models,
    #[command(description = "Set the active model. Usage: /model <model-id>")]
    Model(String),
    #[command(description = "Set the active provider. Usage: /provider <name>")]
    Provider(String),
    #[command(description = "Approve a pending tool action.")]
    Approve,
    #[command(description = "Deny a pending tool action.")]
    Deny,
    #[command(description = "Reset the conversation context.")]
    Reset,
    #[command(description = "Show memory entries for this chat. (Admin)")]
    Memory,
    #[command(description = "Show recent audit log entries. (Admin)")]
    Audit,
    #[command(description = "Show scheduler job status. (Admin)")]
    Jobs,
}
