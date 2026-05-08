function compute(items: Item[]) {
  const prefix = "x";
  const labels = items.map((item) => {
    return prefix + "-" + item.id;
  });
  return labels;
}
