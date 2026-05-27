import { Bot } from "grammy";
import type { Env } from "../types";
import { formatAndSend } from "../telegram/send";

export const createRoute = () => {
  const router = new Router<{ Bindings: Env }>();

  router.post("/webhook", async (c) => {
    const bot = new Bot(c.env.TOKEN);

    const sendReply = async (
      chatId: number,
      threadId: number,
      text: string,
    ) => {
      await formatAndSend(text, (formatted, parseMode) =>
        bot.api.sendMessage(chatId, formatted, {
          message_thread_id: threadId,
          ...(parseMode && { parse_mode: parseMode }),
        }),
      );
    };

    bot.command("new", (ctx) => {
      process(ctx, sendReply);
    });
  });
};
