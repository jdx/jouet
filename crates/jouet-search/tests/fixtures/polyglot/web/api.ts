export interface Api {
  request(path: string): Promise<string>;
}

export class Session {
  constructor(private base: string) {}

  request(path: string): Promise<string> {
    return fetchText(this.base + path);
  }
}

function fetchText(url: string): Promise<string> {
  return Promise.resolve(url);
}
