import { sendHistoryQueryRequest } from "./requests.js";

export async function query(): Promise<Array<Record<string, unknown>>> {
  const response = await sendHistoryQueryRequest({});
  return JSON.parse(response.jsonArray);
}
