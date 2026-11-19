// A month's bill with proven rules: `axiom build --emit js` in examples/invoice,
// then `node app/main.ts` (Node 23.6+ runs TypeScript as is).
import { prorate, supply_of, tax_of, fee, payout } from "../dist/contracts.js";

const plan = 49000n; // 월 요금 (원)
const charged = prorate(plan, 12n, 31n); // 31일 중 12일 사용
const rate = 330n; // PG 수수료 3.3%

console.log("청구액", charged);
console.log("공급가액", supply_of(charged), "세액", tax_of(charged));
console.log("PG 수수료", fee(charged, rate), "정산액", payout(charged, rate));

try {
  prorate(plan, 40n, 31n); // 쓴 날이 달의 날수보다 많음: need 줄이 막음
} catch (error) {
  console.log("거부:", (error as Error).message);
}
