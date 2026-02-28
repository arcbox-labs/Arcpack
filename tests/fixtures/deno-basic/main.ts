const port = parseInt(Deno.env.get("PORT") || "3000");

Deno.serve({ port }, (_req: Request) => {
  return new Response("Hello from Deno!");
});
