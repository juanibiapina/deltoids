class ItemService {
  constructor(
    private readonly db: Database,
    private readonly logger: Logger
  ) {}

  async getById(id: string): Promise<ItemResult> {
    const where = isUuid(id)
      ? { id }
      : { externalId: id };

    const res = await fromPromise(
      this.db.item.findFirst({
        where,
        include: {
          details: true,
        },
      }),
      toError(DbError)
    );

    if (res.isErr()) {
      return err(res.error);
    }

    if (!res.value) {
      return err(NotFoundError(`Item ${id} not found`));
    }

    return ok(res.value).map(toItem);
  }

  async getAll(): Promise<ItemResults> {
    const items = await this.db.item.findMany();
    return ok(items).map((xs) => xs.map(toItem));
  }
}
