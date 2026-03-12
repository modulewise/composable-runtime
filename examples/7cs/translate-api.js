const http = require("http");

const translations = {
  de: { Hello: "Hallo", World: "Welt" },
  es: { Hello: "Hola", World: "Mundo" },
  fr: { Hello: "Bonjour", World: "le Monde" },
  haw: { Hello: "Aloha" },
  nl: { Hello: "Hallo", World: "Wereld" }
};

const server = http.createServer((req, res) => {
  if (req.method !== "POST" || req.url !== "/translate") {
    res.writeHead(404);
    res.end();
    return;
  }

  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    try {
      const { text, locale } = JSON.parse(body);
      const lang = (locale || "").split(/[_-]/)[0];
      const dict = translations[lang] || {};
      const translated = Object.entries(dict).reduce(
        (text, [from, to]) => text.replace(from, to),
        text || ""
      );
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ translated }));
    } catch {
      res.writeHead(400);
      res.end(JSON.stringify({ error: "invalid request" }));
    }
  });
});

const port = process.env.PORT || 8090;
server.listen(port, () => console.log(`Translate API listening on :${port}`));
