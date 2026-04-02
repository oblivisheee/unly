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
    #[command(description = "Set the active model. Usage: /model <model-id>")]
    Model(String),
    #[command(description = "Set the active provider. Usage: /provider <name>")]
    Provider(String),
    #[command(description = "Show subagent capabilities and limits.")]
    Subagent,
    #[command(description = "Show active subagents and statuses.")]
    Subagents,
    #[command(description = "Approve a pending tool action.")]
    Approve,
    #[command(description = "Deny a pending tool action.")]
    Deny,
    #[command(description = "Set approval mode. Usage: /approval <manual|auto>")]
    Approval(String),
    #[command(description = "Reset the conversation context.")]
    Reset,
}
