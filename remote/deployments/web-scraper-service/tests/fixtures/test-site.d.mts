export interface Fixture {
  url: string;
  close(): Promise<void>;
}
export function startFixture(): Promise<Fixture>;
