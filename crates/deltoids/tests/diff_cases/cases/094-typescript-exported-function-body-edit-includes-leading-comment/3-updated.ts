const prefix = "tg";

// Build a TopicContext from a Telegram message.
// Everything else is dropped.
export function resolveContext(chatType: string) {
  if (chatType === "private" || chatType === "group") {
    return 0;
  }
  return null;
}

const suffix = "end";
