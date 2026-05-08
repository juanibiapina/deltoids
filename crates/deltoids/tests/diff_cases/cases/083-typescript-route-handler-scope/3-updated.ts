app.get("/users", (req, res) => {
  const filters = {
    active: true,
    role: "user",
    limit: 50,
  };
  res.json(users.find(filters));
});
