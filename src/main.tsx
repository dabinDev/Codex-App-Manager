import { startRenderer } from "./app/rendererEntry";
import { BRAND } from "./app/brand";

document.title = BRAND.productName;
void startRenderer();
