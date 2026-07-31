const prefix = "tg";

// Build a TopicContext from a Telegram message.
// Everything else is dropped.
export const resolveContext = (chatType: string) => {
  if (chatType === "private") {
    return 0;
  }
  return null;
};

const suffix = "end";
