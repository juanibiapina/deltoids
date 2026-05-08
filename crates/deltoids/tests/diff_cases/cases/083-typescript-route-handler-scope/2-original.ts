app.get("/users", (req, res) => {
  const filters = {
    active: true,
    role: "admin",
    limit: 50,
  };
  res.json(users.find(filters));
});
