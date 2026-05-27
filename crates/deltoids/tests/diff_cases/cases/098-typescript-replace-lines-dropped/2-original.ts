import { Bot } from "grammy";
import type { Env } from "../types";

export const createRoute = () => {
  const router = new Router<{ Bindings: Env }>();

  router.post("/webhook", async (c) => {
    const bot = new Bot(c.env.TOKEN);

    const sendReply = async (
      chatId: number,
      threadId: number,
      text: string,
    ) => {
      await bot.api.sendMessage(chatId, text, {
        message_thread_id: threadId,
      });
    };

    bot.command("new", (ctx) => {
      process(ctx, sendReply);
    });
  });
};
