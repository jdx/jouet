import { Session } from "./api";

function main() {
  const session = new Session("https://example.com");
  return session.request("/index.html");
}

main();
